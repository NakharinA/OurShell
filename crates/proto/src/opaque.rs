//! Shared OPAQUE (password-authenticated key exchange) configuration.
//!
//! Both the client and the server must agree on the same [`CipherSuite`] or
//! the protocol messages won't deserialize. We pick:
//!
//! * `OprfCs` / `KeGroup`: ristretto255 (fast, constant-time, widely audited)
//! * `KeyExchange`: Triple Diffie-Hellman with SHA-512
//! * `Ksf`: Argon2id, so a stolen `opaque_records` row is expensive to
//!   brute-force offline (see `docs/threat-model.md`)
//!
//! `opaque_ke` itself is re-exported so the server can build the
//! server-side registration/login types (`ServerSetup`, `ServerRegistration`,
//! `ServerLogin`, ...) against this exact [`CipherSuite`] without pulling in
//! a second, possibly mismatched, copy of the crate.
//!
//! This module (and the `opaque-ke` / `argon2` / `sha2` / `rand`
//! dependencies it needs) is gated behind the default-on `opaque` cargo
//! feature so a consumer that never talks OPAQUE can drop it.

pub use opaque_ke;

use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse,
};
use rand::rngs::OsRng;

/// The OPAQUE cipher suite shared by every OurShell client and server.
pub struct CipherSuite;

impl opaque_ke::CipherSuite for CipherSuite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, sha2::Sha512>;
    type Ksf = argon2::Argon2<'static>;
}

#[derive(Debug, thiserror::Error)]
pub enum OpaqueError {
    #[error("OPAQUE protocol error: {0}")]
    Protocol(#[from] opaque_ke::errors::ProtocolError),
}

pub type Result<T> = std::result::Result<T, OpaqueError>;

/// Client-side state kept between `client_register_start` and
/// `client_register_finish`. Never leaves the client process.
pub struct ClientRegistrationState(ClientRegistration<CipherSuite>);

/// Client-side state kept between `client_login_start` and
/// `client_login_finish`. Never leaves the client process.
pub struct ClientLoginState(ClientLogin<CipherSuite>);

/// Step 1 of registration: blind the password. Returns opaque client state
/// to keep for the finish step, and the request bytes to send to
/// `POST /v1/register/start`.
pub fn client_register_start(password: &[u8]) -> Result<(ClientRegistrationState, Vec<u8>)> {
    let result = ClientRegistration::<CipherSuite>::start(&mut OsRng, password)?;
    Ok((
        ClientRegistrationState(result.state),
        result.message.serialize().to_vec(),
    ))
}

/// Step 2 of registration: given the server's `RegisterStartResponse.registration_response`,
/// produce the upload bytes for `POST /v1/register/finish` and the export key
/// (which the client may use to encrypt a recovery copy of the account keys,
/// but which is never sent to the server).
pub fn client_register_finish(
    state: ClientRegistrationState,
    password: &[u8],
    response_bytes: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    let response = RegistrationResponse::<CipherSuite>::deserialize(response_bytes)
        .map_err(OpaqueError::Protocol)?;
    let result = state.0.finish(
        &mut OsRng,
        password,
        response,
        ClientRegistrationFinishParameters::default(),
    )?;
    Ok((
        result.message.serialize().to_vec(),
        result.export_key.to_vec(),
    ))
}

/// Step 1 of login: blind the password. Returns opaque client state to keep
/// for the finish step, and the request bytes to send to
/// `POST /v1/login/start`.
pub fn client_login_start(password: &[u8]) -> Result<(ClientLoginState, Vec<u8>)> {
    let result = ClientLogin::<CipherSuite>::start(&mut OsRng, password)?;
    Ok((
        ClientLoginState(result.state),
        result.message.serialize().to_vec(),
    ))
}

/// Step 2 of login: given the server's `LoginStartResponse.credential_response`,
/// produce `(finalization_bytes, session_key, export_key)`. `finalization_bytes`
/// goes to `POST /v1/login/finish`; `session_key` must match the server's
/// session key on success (callers may assert this in tests, the server
/// enforces it); `export_key` unwraps the account keys.
///
/// Returns an error (never a success value) if `password` was wrong.
pub fn client_login_finish(
    state: ClientLoginState,
    password: &[u8],
    response_bytes: &[u8],
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let response = CredentialResponse::<CipherSuite>::deserialize(response_bytes)
        .map_err(OpaqueError::Protocol)?;
    let result = state.0.finish(
        &mut OsRng,
        password,
        response,
        ClientLoginFinishParameters::default(),
    )?;
    Ok((
        result.message.serialize().to_vec(),
        result.session_key.to_vec(),
        result.export_key.to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opaque_ke::{ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup};

    #[test]
    fn full_register_and_login_round_trip() {
        let server_setup = ServerSetup::<CipherSuite>::new(&mut OsRng);
        let identifier = b"alice@example.com";

        // Registration.
        let (client_reg_state, request_bytes) = client_register_start(b"correct horse").unwrap();
        let request =
            opaque_ke::RegistrationRequest::<CipherSuite>::deserialize(&request_bytes).unwrap();
        let server_start =
            ServerRegistration::<CipherSuite>::start(&server_setup, request, identifier).unwrap();
        let response_bytes = server_start.message.serialize().to_vec();
        let (upload_bytes, export_key_reg) =
            client_register_finish(client_reg_state, b"correct horse", &response_bytes).unwrap();
        let upload =
            opaque_ke::RegistrationUpload::<CipherSuite>::deserialize(&upload_bytes).unwrap();
        let password_file = ServerRegistration::<CipherSuite>::finish(upload);

        // Login with the right password succeeds and session keys match.
        let (client_login_state, login_request_bytes) =
            client_login_start(b"correct horse").unwrap();
        let login_request =
            opaque_ke::CredentialRequest::<CipherSuite>::deserialize(&login_request_bytes).unwrap();
        let server_login_start = ServerLogin::start(
            &mut OsRng,
            &server_setup,
            Some(password_file.clone()),
            login_request,
            identifier,
            ServerLoginParameters::default(),
        )
        .unwrap();
        let login_response_bytes = server_login_start.message.serialize().to_vec();
        let (finalization_bytes, client_session_key, export_key_login) =
            client_login_finish(client_login_state, b"correct horse", &login_response_bytes)
                .unwrap();
        assert_eq!(export_key_reg, export_key_login);
        let finalization =
            opaque_ke::CredentialFinalization::<CipherSuite>::deserialize(&finalization_bytes)
                .unwrap();
        let server_login_finish = server_login_start
            .state
            .finish(finalization, ServerLoginParameters::default())
            .unwrap();
        assert_eq!(client_session_key, server_login_finish.session_key.to_vec());
    }

    #[test]
    fn wrong_password_fails_login() {
        let server_setup = ServerSetup::<CipherSuite>::new(&mut OsRng);
        let identifier = b"bob@example.com";

        let (client_reg_state, request_bytes) = client_register_start(b"hunter2").unwrap();
        let request =
            opaque_ke::RegistrationRequest::<CipherSuite>::deserialize(&request_bytes).unwrap();
        let server_start =
            ServerRegistration::<CipherSuite>::start(&server_setup, request, identifier).unwrap();
        let (upload_bytes, _) = client_register_finish(
            client_reg_state,
            b"hunter2",
            &server_start.message.serialize(),
        )
        .unwrap();
        let upload =
            opaque_ke::RegistrationUpload::<CipherSuite>::deserialize(&upload_bytes).unwrap();
        let password_file = ServerRegistration::<CipherSuite>::finish(upload);

        let (client_login_state, login_request_bytes) =
            client_login_start(b"wrong password").unwrap();
        let login_request =
            opaque_ke::CredentialRequest::<CipherSuite>::deserialize(&login_request_bytes).unwrap();
        let server_login_start = ServerLogin::start(
            &mut OsRng,
            &server_setup,
            Some(password_file),
            login_request,
            identifier,
            ServerLoginParameters::default(),
        )
        .unwrap();
        let login_response_bytes = server_login_start.message.serialize().to_vec();
        let result =
            client_login_finish(client_login_state, b"wrong password", &login_response_bytes);
        assert!(result.is_err());
    }
}
