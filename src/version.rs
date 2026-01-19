const VERSION_WITH_GIT: &str = concat!(
    env!("PTYCTL_VERSION_LABEL"),
    " (git ",
    env!("PTYCTL_GIT_SHA"),
    ", built ",
    env!("PTYCTL_BUILD_TIME"),
    ")",
);
const VERSION_NO_GIT: &str = concat!(
    env!("PTYCTL_VERSION_LABEL"),
    " (built ",
    env!("PTYCTL_BUILD_TIME"),
    ")",
);

pub const VERSION: &str = if env!("PTYCTL_GIT_SHA").is_empty() {
    VERSION_NO_GIT
} else {
    VERSION_WITH_GIT
};
