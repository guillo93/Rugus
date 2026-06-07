//! Auto-handshake de autenticación de canal (F6.2) en el cliente.
//!
//! Cuando la consola del dispositivo exige sesión autenticada (feature `auth` en
//! `rush`), el operador debe completar el challenge-response antes de poder usar
//! los verbos privilegiados. Este módulo lo automatiza: dada la PSK, en cuanto se
//! conecta envía `knock`, espera el `challenge <nonce>`, calcula
//! `proof = HMAC-SHA256(PSK, nonce)` con [`rugus_proto::compute_proof_hex`]
//! (mismo HMAC que el firmware) y responde `prove <proof>`.
//!
//! La PSK vive solo en memoria del cliente y nunca se imprime; al cable solo
//! viaja la prueba, nunca el secreto.

use rugus_proto::compute_proof_hex;

/// Estado del auto-handshake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuthState {
    /// Sin PSK: no se intenta autenticar (modo pasivo).
    Disabled,
    /// PSK presente, aún no se ha enviado `knock`.
    Idle,
    /// `knock` enviado, esperando `challenge <nonce>`.
    Knocked,
    /// `prove` enviado, esperando confirmación.
    Proving,
    /// Sesión autenticada (`auth: ok`).
    Authenticated,
    /// Fallo de autenticación (prueba inválida o sin PSK en el dispositivo).
    Failed,
}

/// Acción que el FSM pide al transporte tras procesar una entrada.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AuthAction {
    /// No hay nada que enviar.
    None,
    /// Enviar esta línea de comando al dispositivo (sin terminador).
    Send(String),
    /// Mensaje informativo para el scrollback de la TUI.
    Note(String),
}

/// Máquina de estados del auto-handshake. Reusable y testeable sin transporte.
pub struct AutoAuth {
    /// PSK en bytes (decodificada del hex del argumento), o `None` si pasiva.
    psk: Option<Vec<u8>>,
    state: AuthState,
}

impl AutoAuth {
    /// Crea el FSM. `psk` ya viene decodificada (bytes); `None` => modo pasivo.
    pub fn new(psk: Option<Vec<u8>>) -> Self {
        let state = if psk.is_some() {
            AuthState::Idle
        } else {
            AuthState::Disabled
        };
        Self { psk, state }
    }

    /// Estado actual (para la cabecera de la TUI).
    pub fn state(&self) -> AuthState {
        self.state
    }

    /// `true` si hay PSK y, por tanto, debe intentarse el handshake.
    pub fn is_active(&self) -> bool {
        self.psk.is_some()
    }

    /// Arranca el handshake (al conectar). Devuelve la acción `knock` si procede.
    pub fn start(&mut self) -> AuthAction {
        if self.state == AuthState::Idle {
            self.state = AuthState::Knocked;
            AuthAction::Send("knock".into())
        } else {
            AuthAction::None
        }
    }

    /// Procesa una línea recibida del dispositivo. Si contiene el reto, calcula
    /// la prueba y pide enviar `prove`; si confirma o falla, actualiza el estado.
    ///
    /// La línea puede traer el eco del comando pegado (p. ej. `knockchallenge
    /// 0b36…`), por eso se busca el patrón como subcadena, no anclado.
    pub fn on_line(&mut self, line: &str) -> AuthAction {
        // En modo pasivo no interceptamos nada.
        let Some(psk) = self.psk.as_deref() else {
            return AuthAction::None;
        };

        if self.state == AuthState::Knocked {
            if let Some(nonce) = extract_after(line, "challenge ") {
                let nonce: String = nonce
                    .chars()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .collect();
                return match compute_proof_hex(psk, &nonce) {
                    Some(proof) => {
                        self.state = AuthState::Proving;
                        AuthAction::Send(format!("prove {proof}"))
                    }
                    None => {
                        self.state = AuthState::Failed;
                        AuthAction::Note("auth: nonce inválido en el reto".into())
                    }
                };
            }
            // El dispositivo puede no tener PSK aprovisionada.
            if line.contains("sin PSK") {
                self.state = AuthState::Failed;
                return AuthAction::Note("auth: el dispositivo no tiene PSK (usa `enroll`)".into());
            }
        }

        if self.state == AuthState::Proving {
            if line.contains("auth: ok") {
                self.state = AuthState::Authenticated;
                return AuthAction::Note("auth: sesión autenticada".into());
            }
            if line.contains("auth: fail") {
                self.state = AuthState::Failed;
                return AuthAction::Note("auth: prueba rechazada (PSK incorrecta)".into());
            }
        }

        AuthAction::None
    }

    /// Etiqueta corta del estado para la cabecera.
    pub fn label(&self) -> &'static str {
        match self.state {
            AuthState::Disabled => "auth: off",
            AuthState::Idle => "auth: …",
            AuthState::Knocked => "auth: knock",
            AuthState::Proving => "auth: prove",
            AuthState::Authenticated => "auth: ✓",
            AuthState::Failed => "auth: ✗",
        }
    }
}

/// Devuelve la subcadena que sigue a la primera aparición de `needle`, o `None`.
fn extract_after<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    haystack.find(needle).map(|i| &haystack[i + needle.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    // PSK de prueba (misma que la validación HW del F103).
    fn psk() -> Vec<u8> {
        rugus_proto::decode_hex("00112233445566778899aabbccddeeff").unwrap()
    }

    #[test]
    fn passive_without_psk() {
        let mut a = AutoAuth::new(None);
        assert_eq!(a.state(), AuthState::Disabled);
        assert!(!a.is_active());
        assert_eq!(a.start(), AuthAction::None);
        assert_eq!(a.on_line("challenge abcd"), AuthAction::None);
    }

    #[test]
    fn full_handshake() {
        let mut a = AutoAuth::new(Some(psk()));
        assert!(a.is_active());
        assert_eq!(a.start(), AuthAction::Send("knock".into()));
        // Reto con eco del comando pegado, como en la consola real.
        let nonce = "0b366bed78458bd3f2dcbfe822855057";
        let expected = compute_proof_hex(&psk(), nonce).unwrap();
        assert_eq!(
            a.on_line(&format!("knockchallenge {nonce}")),
            AuthAction::Send(format!("prove {expected}"))
        );
        assert_eq!(a.state(), AuthState::Proving);
        match a.on_line("prove ...auth: ok — sesión abierta") {
            AuthAction::Note(_) => {}
            other => panic!("esperaba Note, fue {other:?}"),
        }
        assert_eq!(a.state(), AuthState::Authenticated);
    }

    #[test]
    fn rejected_proof() {
        let mut a = AutoAuth::new(Some(psk()));
        a.start();
        a.on_line("challenge 0b366bed78458bd3f2dcbfe822855057");
        a.on_line("auth: fail — prueba inválida");
        assert_eq!(a.state(), AuthState::Failed);
    }

    #[test]
    fn device_without_psk() {
        let mut a = AutoAuth::new(Some(psk()));
        a.start();
        let act = a.on_line("auth: sin PSK — usa enroll <psk-hex> (una vez)");
        assert!(matches!(act, AuthAction::Note(_)));
        assert_eq!(a.state(), AuthState::Failed);
    }

    #[test]
    fn knock_only_once() {
        let mut a = AutoAuth::new(Some(psk()));
        assert_eq!(a.start(), AuthAction::Send("knock".into()));
        assert_eq!(a.start(), AuthAction::None);
    }
}
