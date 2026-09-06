use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;
use crate::remote_control::RemoteControlEnableError;
use crate::remote_control::RemoteControlHandle;
use crate::remote_control::RemoteControlUnavailable;
use code_app_server_protocol::RemoteControlClientsListParams;
use code_app_server_protocol::RemoteControlClientsListResponse;
use code_app_server_protocol::RemoteControlClientsRevokeParams;
use code_app_server_protocol::RemoteControlClientsRevokeResponse;
use code_app_server_protocol::RemoteControlDisableResponse;
use code_app_server_protocol::RemoteControlEnableResponse;
use code_app_server_protocol::RemoteControlPairingStartParams;
use code_app_server_protocol::RemoteControlPairingStartResponse;
use code_app_server_protocol::RemoteControlPairingStatusParams;
use code_app_server_protocol::RemoteControlPairingStatusResponse;
use code_app_server_protocol::RemoteControlStatusReadResponse;
use code_app_server_protocol::RemoteControlStatusChangedNotification;
use mcp_types::JSONRPCErrorError;
use std::io;

#[derive(Clone)]
pub(crate) struct RemoteControlRequestProcessor {
    remote_control_handle: Option<RemoteControlHandle>,
}

impl RemoteControlRequestProcessor {
    pub(crate) fn new(remote_control_handle: Option<RemoteControlHandle>) -> Self {
        Self {
            remote_control_handle,
        }
    }

    pub(crate) async fn enable(
        &self,
        ephemeral: bool,
        app_server_client_name: Option<&str>,
    ) -> Result<RemoteControlEnableResponse, JSONRPCErrorError> {
        let handle = self.handle()?;
        let status = if ephemeral {
            handle.enable_ephemeral().map_err(map_enable_error)?
        } else {
            handle
                .enable(app_server_client_name)
                .await
                .map_err(map_update_error)?
        };
        Ok(RemoteControlEnableResponse::from(status))
    }

    pub(crate) async fn disable(
        &self,
        ephemeral: bool,
        app_server_client_name: Option<&str>,
    ) -> Result<RemoteControlDisableResponse, JSONRPCErrorError> {
        let handle = self.handle()?;
        let status = if ephemeral {
            handle.disable_ephemeral().await
        } else {
            handle
                .disable(app_server_client_name)
                .await
                .map_err(map_update_error)?
        };
        Ok(RemoteControlDisableResponse::from(status))
    }

    pub(crate) fn status_read(&self) -> Result<RemoteControlStatusReadResponse, JSONRPCErrorError> {
        let status = self.handle()?.status();
        Ok(RemoteControlStatusReadResponse {
            status: status.status,
            server_name: status.server_name,
            installation_id: status.installation_id,
            environment_id: status.environment_id,
        })
    }

    pub(crate) fn status_notification(&self) -> Option<RemoteControlStatusChangedNotification> {
        self.remote_control_handle.as_ref().map(RemoteControlHandle::status)
    }

    pub(crate) async fn resolve_persisted_preference(
        &self,
        app_server_client_name: Option<&str>,
    ) -> io::Result<bool> {
        let Some(handle) = self.remote_control_handle.as_ref() else {
            return Ok(false);
        };
        handle
            .resolve_persisted_preference(app_server_client_name)
            .await
    }

    pub(crate) async fn pairing_start(
        &self,
        params: RemoteControlPairingStartParams,
        app_server_client_name: Option<&str>,
    ) -> Result<RemoteControlPairingStartResponse, JSONRPCErrorError> {
        self.handle()?
            .start_pairing(params, app_server_client_name)
            .await
            .map_err(map_pairing_error)
    }

    pub(crate) async fn pairing_status(
        &self,
        params: RemoteControlPairingStatusParams,
    ) -> Result<RemoteControlPairingStatusResponse, JSONRPCErrorError> {
        validate_pairing_status_params(&params)?;
        self.handle()?
            .pairing_status(params)
            .await
            .map_err(map_pairing_error)
    }

    pub(crate) async fn clients_list(
        &self,
        params: RemoteControlClientsListParams,
    ) -> Result<RemoteControlClientsListResponse, JSONRPCErrorError> {
        self.handle()?
            .list_clients(params)
            .await
            .map_err(map_client_management_error)
    }

    pub(crate) async fn clients_revoke(
        &self,
        params: RemoteControlClientsRevokeParams,
    ) -> Result<RemoteControlClientsRevokeResponse, JSONRPCErrorError> {
        self.handle()?
            .revoke_client(params)
            .await
            .map_err(map_client_management_error)
    }

    fn handle(&self) -> Result<&RemoteControlHandle, JSONRPCErrorError> {
        let handle = self.remote_control_handle.as_ref().ok_or_else(|| {
            internal_error("remote control is unavailable for this app-server")
        })?;
        handle
            .ensure_remote_control_allowed()
            .map_err(|error| invalid_request(error.to_string()))?;
        Ok(handle)
    }
}

fn map_enable_error(error: RemoteControlEnableError) -> JSONRPCErrorError {
    match error {
        RemoteControlEnableError::Unavailable(error) => map_unavailable(error),
        RemoteControlEnableError::DisabledByRequirements(error) => {
            invalid_request(error.to_string())
        }
    }
}

fn map_unavailable(error: RemoteControlUnavailable) -> JSONRPCErrorError {
    invalid_request(error.to_string())
}

fn map_update_error(error: io::Error) -> JSONRPCErrorError {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    ) {
        invalid_request(error.to_string())
    } else {
        internal_error(error.to_string())
    }
}

pub(crate) fn map_pairing_error(error: io::Error) -> JSONRPCErrorError {
    if error.kind() == io::ErrorKind::InvalidInput {
        invalid_request(error.to_string())
    } else {
        internal_error(error.to_string())
    }
}

pub(crate) fn validate_pairing_status_params(
    params: &RemoteControlPairingStatusParams,
) -> Result<(), JSONRPCErrorError> {
    match (&params.pairing_code, &params.manual_pairing_code) {
        (Some(_), None) | (None, Some(_)) => Ok(()),
        (Some(_), Some(_)) => Err(invalid_request(
            "remoteControl/pairing/status accepts either pairingCode or manualPairingCode, not both",
        )),
        (None, None) => Err(invalid_request(
            "remoteControl/pairing/status requires pairingCode or manualPairingCode",
        )),
    }
}

pub(crate) fn map_client_management_error(error: io::Error) -> JSONRPCErrorError {
    match error.kind() {
        io::ErrorKind::InvalidInput
        | io::ErrorKind::NotFound
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::WouldBlock => invalid_request(error.to_string()),
        _ => internal_error(error.to_string()),
    }
}

fn invalid_request(message: impl Into<String>) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INVALID_REQUEST_ERROR_CODE,
        message: message.into(),
        data: None,
    }
}

fn internal_error(message: impl Into<String>) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INTERNAL_ERROR_CODE,
        message: message.into(),
        data: None,
    }
}
