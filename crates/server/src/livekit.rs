//! LiveKit room tokens: an HS256 JWT signed with the project's API secret, granting room join
//! and dispatching the voice agent into the room.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

const TOKEN_TTL_SECS: i64 = 6 * 60 * 60;

pub struct RoomToken<'a> {
    pub api_key: &'a str,
    pub api_secret: &'a str,
    pub identity: &'a str,
    pub name: &'a str,
    pub room: &'a str,
    pub agent_name: &'a str,
}

impl RoomToken<'_> {
    pub fn sign(&self, now: i64) -> String {
        let header = json!({ "alg": "HS256", "typ": "JWT" });
        let claims = json!({
            "iss": self.api_key,
            "sub": self.identity,
            "name": self.name,
            "nbf": now,
            "exp": now + TOKEN_TTL_SECS,
            "video": {
                "roomJoin": true,
                "room": self.room,
                "canPublish": true,
                "canSubscribe": true,
                "canPublishData": true,
            },
            "roomConfig": {
                "agents": [{ "agentName": self.agent_name }],
            },
        });
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(self.api_secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(signing_input.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{signing_input}.{signature}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_a_verifiable_jwt() {
        let token = RoomToken {
            api_key: "key",
            api_secret: "secret",
            identity: "user-octocat",
            name: "octocat",
            room: "blue-room",
            agent_name: "blue-voice",
        }
        .sign(1_700_000_000);
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap())
            .unwrap();

        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["video"]["room"], "blue-room");
        assert_eq!(claims["roomConfig"]["agents"][0]["agentName"], "blue-voice");
        assert_eq!(claims["exp"], 1_700_000_000 + TOKEN_TTL_SECS);
    }
}
