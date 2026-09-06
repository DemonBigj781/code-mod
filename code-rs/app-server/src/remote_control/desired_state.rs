#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteControlDesiredState {
    Unknown,
    Disabled,
    Enabled {
        persistence_preference: Option<bool>,
    },
}

impl RemoteControlDesiredState {
    pub(crate) fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    pub(crate) fn persistence_preference(self) -> Option<bool> {
        match self {
            Self::Enabled {
                persistence_preference,
            } => persistence_preference,
            Self::Unknown | Self::Disabled => None,
        }
    }
}
