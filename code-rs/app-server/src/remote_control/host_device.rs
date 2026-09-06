#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostDevice {
    pub name: String,
    pub os: String,
    pub arch: String,
    pub device_kind: Option<String>,
}

impl HostDevice {
    pub fn detect(server_name: String) -> Self {
        Self {
            name: server_name,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            device_kind: None,
        }
    }

    #[cfg(test)]
    pub(super) fn for_testing(name: &str, os: &str, arch: &str, device_kind: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            os: os.to_string(),
            arch: arch.to_string(),
            device_kind: device_kind.map(str::to_string),
        }
    }
}
