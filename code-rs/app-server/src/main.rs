use clap::Parser;
use code_app_server::AppServerRuntimeOptions;
use code_app_server::AppServerTransport;
use code_app_server::RemoteControlStartupMode;
use code_app_server::run_main_with_transport_options;
use code_arg0::arg0_dispatch_or_else;
use code_common::CliConfigOverrides;

#[derive(Debug, Parser)]
struct AppServerArgs {
    #[command(flatten)]
    config_overrides: CliConfigOverrides,

    /// Transport endpoint URL. Supported values: `stdio://` (default),
    /// `unix:///absolute/path`, `ws://IP:PORT`, or `off://`.
    #[arg(
        long = "listen",
        value_name = "URL",
        default_value = AppServerTransport::DEFAULT_LISTEN_URL
    )]
    listen: AppServerTransport,

    /// Enable remote control for this process without changing persistence.
    #[arg(long = "remote-control", hide = true)]
    remote_control: bool,
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|code_linux_sandbox_exe| async move {
        let args = AppServerArgs::parse();
        let runtime_options = AppServerRuntimeOptions {
            remote_control_startup_mode: if args.remote_control {
                RemoteControlStartupMode::EnabledEphemeral
            } else {
                RemoteControlStartupMode::ResolvePersisted
            },
            ..Default::default()
        };
        run_main_with_transport_options(
            code_linux_sandbox_exe,
            args.config_overrides,
            args.listen,
            runtime_options,
        )
        .await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_private_remote_control_runtime_options() {
        let args = AppServerArgs::try_parse_from([
            "code-app-server",
            "--listen",
            "off://",
            "--remote-control",
            "-c",
            "model=test-model",
        ])
        .expect("runtime options should parse");

        assert_eq!(args.listen, AppServerTransport::Off);
        assert!(args.remote_control);
        assert_eq!(
            args.config_overrides.raw_overrides,
            vec!["model=test-model".to_owned()]
        );
    }
}
