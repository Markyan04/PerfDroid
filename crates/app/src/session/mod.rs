use pdcore::types::ControlCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Disconnected,
    Connected,
    Running,
    Paused,
    Stopped,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connected => "Connected",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Stopped => "Stopped",
        }
    }

    pub fn allows(self, command: ControlCommand) -> bool {
        matches!(
            (self, command),
            (Self::Disconnected, ControlCommand::Connect)
                | (Self::Connected, ControlCommand::Start)
                | (Self::Connected, ControlCommand::Stop)
                | (Self::Running, ControlCommand::Pause)
                | (Self::Running, ControlCommand::Stop)
                | (Self::Paused, ControlCommand::Restart)
                | (Self::Paused, ControlCommand::Stop)
                | (Self::Stopped, ControlCommand::Connect)
        )
    }

    pub fn next(self, command: ControlCommand) -> Option<Self> {
        match (self, command) {
            (Self::Disconnected, ControlCommand::Connect) => Some(Self::Connected),
            (Self::Connected, ControlCommand::Start) => Some(Self::Running),
            (Self::Connected, ControlCommand::Stop) => Some(Self::Stopped),
            (Self::Running, ControlCommand::Pause) => Some(Self::Paused),
            (Self::Running, ControlCommand::Stop) => Some(Self::Stopped),
            (Self::Paused, ControlCommand::Restart) => Some(Self::Running),
            (Self::Paused, ControlCommand::Stop) => Some(Self::Stopped),
            (Self::Stopped, ControlCommand::Connect) => Some(Self::Connected),
            _ => None,
        }
    }
}
