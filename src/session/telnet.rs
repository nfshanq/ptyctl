use crate::error::{ApiError, ErrorCode, PtyResult};
use crate::session::{OutputHandle, PtyOptions, SessionBackend};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};

const IAC: u8 = 0xff;
const DONT: u8 = 0xfe;
const DO: u8 = 0xfd;
const WONT: u8 = 0xfc;
const WILL: u8 = 0xfb;
const SB: u8 = 0xfa;
const SE: u8 = 0xf0;

const OPT_BINARY: u8 = 0;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

enum WriteItem {
    Data(Vec<u8>),
    Raw(Vec<u8>),
    Close,
}

pub struct TelnetBackend {
    sender: mpsc::Sender<WriteItem>,
    eof: Arc<AtomicBool>,
    negotiator: Arc<Mutex<Negotiator>>,
}

impl TelnetBackend {
    pub async fn connect(
        host: &str,
        port: u16,
        pty: PtyOptions,
        connect_timeout_ms: u64,
        output: OutputHandle,
    ) -> PtyResult<Self> {
        let addr = format!("{}:{}", host, port);
        let stream = timeout(
            Duration::from_millis(connect_timeout_ms),
            TcpStream::connect(addr),
        )
        .await
        .map_err(|_| ApiError::new(ErrorCode::ConnectTimeout, "Telnet connect timeout"))?
        .map_err(|err| {
            ApiError::new(ErrorCode::ConnectFailed, "Telnet connect failed")
                .with_details(err.to_string())
        })?;

        let (reader, writer) = stream.into_split();
        let (tx, mut rx) = mpsc::channel::<WriteItem>(128);
        let eof = Arc::new(AtomicBool::new(false));
        let eof_flag = eof.clone();

        tokio::spawn(async move {
            let mut writer = writer;
            while let Some(item) = rx.recv().await {
                match item {
                    WriteItem::Close => {
                        let _ = writer.shutdown().await;
                        break;
                    }
                    WriteItem::Data(data) => {
                        let payload = escape_iac(&data);
                        if let Err(err) = writer.write_all(&payload).await {
                            tracing::warn!(error = %err, "Telnet write failed");
                            break;
                        }
                        let _ = writer.flush().await;
                    }
                    WriteItem::Raw(data) => {
                        if let Err(err) = writer.write_all(&data).await {
                            tracing::warn!(error = %err, "Telnet write failed");
                            break;
                        }
                        let _ = writer.flush().await;
                    }
                }
            }
        });

        let negotiator = Arc::new(Mutex::new(Negotiator::new(
            pty.term.clone(),
            pty.cols,
            pty.rows,
        )));
        let negotiator_clone = negotiator.clone();
        let output_clone = output.clone();
        let mut parser = TelnetParser::default();
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let result = parser.process(&buf[..n]);
                        if !result.data.is_empty() {
                            output_clone.append_output(&result.data);
                        }
                        for event in result.events {
                            let responses = {
                                let mut negotiator =
                                    negotiator_clone.lock().expect("negotiator mutex poisoned");
                                negotiator.handle_event(event)
                            };
                            for response in responses {
                                let _ = tx_clone.send(WriteItem::Raw(response)).await;
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "Telnet read failed");
                        break;
                    }
                }
            }
            eof_flag.store(true, Ordering::SeqCst);
            output_clone.append_output(b"");
        });

        Ok(Self {
            sender: tx,
            eof,
            negotiator,
        })
    }
}

#[async_trait]
impl SessionBackend for TelnetBackend {
    async fn write(&self, data: &[u8]) -> PtyResult<usize> {
        let payload = data.to_vec();
        self.sender
            .send(WriteItem::Data(payload))
            .await
            .map_err(|_| ApiError::new(ErrorCode::IoError, "Telnet write failed"))?;
        Ok(data.len())
    }

    async fn resize(&self, cols: u16, rows: u16) -> PtyResult<()> {
        let response = {
            let mut negotiator = self.negotiator.lock().expect("negotiator mutex poisoned");
            negotiator.set_window_size(cols, rows)
        };
        if let Some(response) = response {
            self.sender
                .send(WriteItem::Raw(response))
                .await
                .map_err(|_| ApiError::new(ErrorCode::IoError, "Telnet resize failed"))?;
        }
        Ok(())
    }

    async fn close(&self, _force: bool) -> PtyResult<()> {
        self.sender
            .send(WriteItem::Close)
            .await
            .map_err(|_| ApiError::new(ErrorCode::IoError, "Telnet close failed"))?;
        Ok(())
    }

    fn is_eof(&self) -> bool {
        self.eof.load(Ordering::SeqCst)
    }
}

struct ParseResult {
    data: Vec<u8>,
    events: Vec<TelnetEvent>,
}

#[derive(Debug)]
enum TelnetEvent {
    Negotiation { command: u8, option: u8 },
    Subnegotiation { option: u8, data: Vec<u8> },
}

#[derive(Default)]
struct TelnetParser {
    state: ParserState,
    sb_option: Option<u8>,
    sb_data: Vec<u8>,
}

#[derive(Debug, Default)]
enum ParserState {
    #[default]
    Data,
    Iac,
    Command(u8),
    Subnegotiation,
    SubIac,
}

impl TelnetParser {
    fn process(&mut self, input: &[u8]) -> ParseResult {
        let mut data = Vec::new();
        let mut events = Vec::new();

        for &byte in input {
            match self.state {
                ParserState::Data => {
                    if byte == IAC {
                        self.state = ParserState::Iac;
                    } else {
                        data.push(byte);
                    }
                }
                ParserState::Iac => match byte {
                    IAC => {
                        data.push(IAC);
                        self.state = ParserState::Data;
                    }
                    DO | DONT | WILL | WONT => {
                        self.state = ParserState::Command(byte);
                    }
                    SB => {
                        self.state = ParserState::Subnegotiation;
                        self.sb_option = None;
                        self.sb_data.clear();
                    }
                    SE => {
                        self.state = ParserState::Data;
                    }
                    _ => {
                        self.state = ParserState::Data;
                    }
                },
                ParserState::Command(cmd) => {
                    events.push(TelnetEvent::Negotiation {
                        command: cmd,
                        option: byte,
                    });
                    self.state = ParserState::Data;
                }
                ParserState::Subnegotiation => {
                    if byte == IAC {
                        self.state = ParserState::SubIac;
                    } else if self.sb_option.is_none() {
                        self.sb_option = Some(byte);
                    } else {
                        self.sb_data.push(byte);
                    }
                }
                ParserState::SubIac => {
                    if byte == SE {
                        if let Some(option) = self.sb_option.take() {
                            events.push(TelnetEvent::Subnegotiation {
                                option,
                                data: self.sb_data.clone(),
                            });
                        }
                        self.sb_data.clear();
                        self.state = ParserState::Data;
                    } else if byte == IAC {
                        self.sb_data.push(IAC);
                        self.state = ParserState::Subnegotiation;
                    } else {
                        self.state = ParserState::Subnegotiation;
                    }
                }
            }
        }

        ParseResult { data, events }
    }
}

struct Negotiator {
    local_enabled: HashMap<u8, bool>,
    remote_enabled: HashMap<u8, bool>,
    term: String,
    cols: u16,
    rows: u16,
}

impl Negotiator {
    fn new(term: String, cols: u16, rows: u16) -> Self {
        Self {
            local_enabled: HashMap::new(),
            remote_enabled: HashMap::new(),
            term,
            cols,
            rows,
        }
    }

    fn handle_event(&mut self, event: TelnetEvent) -> Vec<Vec<u8>> {
        match event {
            TelnetEvent::Negotiation { command, option } => {
                let mut responses = Vec::new();
                match command {
                    DO => {
                        if self.allow_local(option) {
                            if !self.is_local_enabled(option) {
                                self.set_local(option, true);
                                responses.push(iac_command(WILL, option));
                                if option == OPT_NAWS {
                                    responses.push(self.build_naws());
                                }
                            }
                        } else if self.is_local_enabled(option) {
                            self.set_local(option, false);
                            responses.push(iac_command(WONT, option));
                        } else {
                            responses.push(iac_command(WONT, option));
                        }
                    }
                    DONT => {
                        if self.is_local_enabled(option) {
                            self.set_local(option, false);
                            responses.push(iac_command(WONT, option));
                        }
                    }
                    WILL => {
                        if self.allow_remote(option) {
                            if !self.is_remote_enabled(option) {
                                self.set_remote(option, true);
                                responses.push(iac_command(DO, option));
                            }
                        } else if self.is_remote_enabled(option) {
                            self.set_remote(option, false);
                            responses.push(iac_command(DONT, option));
                        } else {
                            responses.push(iac_command(DONT, option));
                        }
                    }
                    WONT => {
                        if self.is_remote_enabled(option) {
                            self.set_remote(option, false);
                            responses.push(iac_command(DONT, option));
                        }
                    }
                    _ => {}
                }
                responses
            }
            TelnetEvent::Subnegotiation { option, data } => {
                self.handle_subnegotiation(option, data)
            }
        }
    }

    fn handle_subnegotiation(&mut self, option: u8, data: Vec<u8>) -> Vec<Vec<u8>> {
        if option == OPT_TTYPE && data.first().copied() == Some(TTYPE_SEND) {
            return vec![self.build_ttype()];
        }
        Vec::new()
    }

    fn allow_local(&self, option: u8) -> bool {
        matches!(option, OPT_BINARY | OPT_SGA | OPT_TTYPE | OPT_NAWS)
    }

    fn allow_remote(&self, option: u8) -> bool {
        matches!(option, OPT_BINARY | OPT_ECHO | OPT_SGA)
    }

    fn is_local_enabled(&self, option: u8) -> bool {
        self.local_enabled.get(&option).copied().unwrap_or(false)
    }

    fn is_remote_enabled(&self, option: u8) -> bool {
        self.remote_enabled.get(&option).copied().unwrap_or(false)
    }

    fn set_local(&mut self, option: u8, enabled: bool) {
        self.local_enabled.insert(option, enabled);
    }

    fn set_remote(&mut self, option: u8, enabled: bool) {
        self.remote_enabled.insert(option, enabled);
    }

    fn build_naws(&self) -> Vec<u8> {
        let mut payload = vec![IAC, SB, OPT_NAWS];
        let width = self.cols.to_be_bytes();
        let height = self.rows.to_be_bytes();
        payload.extend_from_slice(&escape_iac_bytes(&width));
        payload.extend_from_slice(&escape_iac_bytes(&height));
        payload.extend_from_slice(&[IAC, SE]);
        payload
    }

    fn build_ttype(&self) -> Vec<u8> {
        let mut payload = vec![IAC, SB, OPT_TTYPE, TTYPE_IS];
        payload.extend_from_slice(&escape_iac_bytes(self.term.as_bytes()));
        payload.extend_from_slice(&[IAC, SE]);
        payload
    }

    fn set_window_size(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        self.cols = cols;
        self.rows = rows;
        if self.is_local_enabled(OPT_NAWS) {
            Some(self.build_naws())
        } else {
            None
        }
    }
}

fn iac_command(cmd: u8, option: u8) -> Vec<u8> {
    vec![IAC, cmd, option]
}

fn escape_iac_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if b == IAC {
            out.push(IAC);
            out.push(IAC);
        } else {
            out.push(b);
        }
    }
    out
}

fn escape_iac(bytes: &[u8]) -> Vec<u8> {
    escape_iac_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_iac_iac() {
        let mut parser = TelnetParser::default();
        let result = parser.process(&[IAC, IAC, b'A']);
        assert_eq!(result.data, vec![IAC, b'A']);
    }

    #[test]
    fn parser_handles_split_subnegotiation() {
        let mut parser = TelnetParser::default();
        let part1 = parser.process(&[IAC, SB, OPT_TTYPE, TTYPE_SEND]);
        assert!(part1.events.is_empty());
        let part2 = parser.process(&[IAC, SE]);
        assert_eq!(part2.events.len(), 1);
        match &part2.events[0] {
            TelnetEvent::Subnegotiation { option, data } => {
                assert_eq!(*option, OPT_TTYPE);
                assert_eq!(data, &vec![TTYPE_SEND]);
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn negotiator_responds_to_ttype_send() {
        let mut negotiator = Negotiator::new("xterm".to_string(), 80, 24);
        let responses = negotiator.handle_event(TelnetEvent::Subnegotiation {
            option: OPT_TTYPE,
            data: vec![TTYPE_SEND],
        });
        assert_eq!(responses.len(), 1);
        assert!(responses[0].starts_with(&[IAC, SB, OPT_TTYPE, TTYPE_IS]));
    }

    #[test]
    fn negotiator_accepts_naws() {
        let mut negotiator = Negotiator::new("xterm".to_string(), 80, 24);
        let responses = negotiator.handle_event(TelnetEvent::Negotiation {
            command: DO,
            option: OPT_NAWS,
        });
        assert!(
            responses
                .iter()
                .any(|resp| resp.starts_with(&[IAC, WILL, OPT_NAWS]))
        );
        assert!(
            responses
                .iter()
                .any(|resp| resp.starts_with(&[IAC, SB, OPT_NAWS]))
        );
    }
}
