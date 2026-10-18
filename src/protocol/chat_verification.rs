//! Remote chat certificates and messages, pinned to 26.1.2 ProfilePublicKey and signed links.
//! Network fetching belongs to the profile resolver; every expiry decision uses caller-observed time.
use crate::{
    entities::RemoteChatKey,
    profile_textures::{MojangServicesKeys, ServiceKeySet},
    ui::{ChatReceipt, ChatVerification},
};
use rsa::{
    RsaPublicKey,
    pkcs1v15::{Signature, VerifyingKey},
    pkcs8::DecodePublicKey,
    signature::hazmat::PrehashVerifier,
    traits::PublicKeyParts,
};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;
const KEY_GRACE_MILLIS: i64 = 8 * 60 * 60 * 1000;
const MESSAGE_CLIENT_LIFETIME: i64 = 7 * 60 * 1000;
const MAX_SENDERS: usize = 100_000;
struct Session {
    profile_generation: u64,
    key_fingerprint: [u8; 32],
    services_revision: u64,
    session_id: Uuid,
    expires_at: i64,
    public_key: VerifyingKey<Sha256>,
    failure: Option<ChatVerification>,
    last: Option<(i32, [u8; 32])>,
}
/// One verifier per connection; clear on teardown, remove on PlayerInfo removal. A changed
/// profile/session/certificate resets only that sender's source SignedMessageValidator chain.
pub struct ChatVerifier {
    keys: MojangServicesKeys,
    sessions: BTreeMap<Uuid, Session>,
}
impl ChatVerifier {
    pub fn new(keys: MojangServicesKeys) -> Self {
        Self {
            keys,
            sessions: BTreeMap::new(),
        }
    }
    pub fn clear(&mut self) {
        self.sessions.clear();
    }
    pub fn remove_player(&mut self, uuid: &str) {
        if let Ok(uuid) = Uuid::parse_str(uuid) {
            self.sessions.remove(&uuid);
        }
    }
    pub fn verify(
        &mut self,
        receipt: &ChatReceipt,
        key: Option<&RemoteChatKey>,
        profile_generation: u64,
        now_millis: i64,
    ) -> ChatVerification {
        let Ok(sender) = Uuid::parse_str(&receipt.sender) else {
            return ChatVerification::InvalidSignature;
        };
        let Some(key) = key else {
            self.sessions.remove(&sender);
            return if receipt.signature.is_none() {
                ChatVerification::Unsigned
            } else {
                ChatVerification::MissingKey
            };
        };
        let Some(services) = self.keys.snapshot() else {
            return ChatVerification::MissingKey;
        };
        if services.player_certificate.is_empty() {
            return ChatVerification::MissingKey;
        }
        let fingerprint = key_fingerprint(key);
        let replace = self.sessions.get(&sender).is_none_or(|session| {
            session.profile_generation != profile_generation
                || session.key_fingerprint != fingerprint
        });
        if replace {
            let session = match validate_certificate(sender, key, &services) {
                Ok((public_key, session_id, expires_at)) => Session {
                    profile_generation,
                    key_fingerprint: fingerprint,
                    services_revision: services.revision,
                    session_id,
                    expires_at,
                    public_key: VerifyingKey::new(public_key),
                    failure: None,
                    last: None,
                },
                Err(status) => {
                    self.sessions.remove(&sender);
                    return status;
                }
            };
            if self.sessions.len() >= MAX_SENDERS && !self.sessions.contains_key(&sender) {
                return ChatVerification::NotVerified;
            }
            self.sessions.insert(sender, session);
        }
        if self
            .sessions
            .get(&sender)
            .is_some_and(|session| session.services_revision != services.revision)
        {
            if let Err(status) = validate_certificate(sender, key, &services) {
                self.sessions.remove(&sender);
                return status;
            }
            self.sessions.get_mut(&sender).unwrap().services_revision = services.revision;
        }
        let session = self.sessions.get_mut(&sender).unwrap();
        // Source isChainValid is sticky for a given remote session, even after a later good signature.
        if let Some(failure) = session.failure {
            return failure;
        }
        if now_millis > session.expires_at.saturating_add(KEY_GRACE_MILLIS) {
            session.failure = Some(ChatVerification::ExpiredKey);
            return ChatVerification::ExpiredKey;
        }
        let Some(signature) = &receipt.signature else {
            session.failure = Some(ChatVerification::InvalidSignature);
            return ChatVerification::Unsigned;
        };
        let digest = match message_digest(receipt, sender, session.session_id) {
            Some(digest) if signature.len() == 256 => digest,
            _ => {
                session.failure = Some(ChatVerification::InvalidSignature);
                return ChatVerification::InvalidSignature;
            }
        };
        let Ok(parsed_signature) = Signature::try_from(signature.as_ref()) else {
            session.failure = Some(ChatVerification::InvalidSignature);
            return ChatVerification::InvalidSignature;
        };
        if session
            .public_key
            .verify_prehash(&digest, &parsed_signature)
            .is_err()
        {
            session.failure = Some(ChatVerification::InvalidSignature);
            return ChatVerification::InvalidSignature;
        }
        let identity = message_identity(receipt, digest, signature);
        if let Some((last_index, last_identity)) = &session.last {
            // Source permits the identical last PlayerChatMessage; otherwise index must increase,
            // not necessarily by one (filtered/delayed messages can leave gaps).
            if receipt.index <= *last_index && identity != *last_identity {
                session.failure = Some(ChatVerification::InvalidSignature);
                return ChatVerification::InvalidSignature;
            }
        }
        session.last = Some((receipt.index, identity));
        if now_millis > receipt.timestamp.saturating_add(MESSAGE_CLIENT_LIFETIME) {
            ChatVerification::NotVerified
        } else {
            ChatVerification::Verified
        }
    }
}
fn key_fingerprint(key: &RemoteChatKey) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update((key.session_id.len() as u64).to_be_bytes());
    digest.update(key.session_id.as_bytes());
    digest.update((key.expires_at_millis.len() as u64).to_be_bytes());
    digest.update(key.expires_at_millis.as_bytes());
    digest.update((key.public_key_der.len() as u64).to_be_bytes());
    digest.update(&key.public_key_der);
    digest.update(&key.signature);
    digest.finalize().into()
}
fn validate_certificate(
    sender: Uuid,
    key: &RemoteChatKey,
    services: &ServiceKeySet,
) -> std::result::Result<(RsaPublicKey, Uuid, i64), ChatVerification> {
    let invalid = ChatVerification::InvalidCertificate;
    if key.public_key_der.len() > 512 || key.signature.len() > 4096 || key.signature.is_empty() {
        return Err(invalid);
    }
    let session_id = Uuid::parse_str(&key.session_id).map_err(|_| invalid)?;
    let expires_at = key.expires_at_millis.parse::<i64>().map_err(|_| invalid)?;
    let public_key = RsaPublicKey::from_public_key_der(&key.public_key_der).map_err(|_| invalid)?;
    // Protocol MessageSignature is 256 bytes, so the remote message signing key is RSA-2048.
    if public_key.n().bits() != 2048 {
        return Err(invalid);
    }
    let mut digest = Sha1::new();
    digest.update(sender.as_bytes());
    digest.update(expires_at.to_be_bytes());
    digest.update(&key.public_key_der);
    let digest = digest.finalize();
    let signature = Signature::try_from(key.signature.as_ref()).map_err(|_| invalid)?;
    if !services
        .player_certificate
        .iter()
        .any(|service| service.verify_prehash(&digest, &signature).is_ok())
    {
        return Err(invalid);
    }
    Ok((public_key, session_id, expires_at))
}
fn message_digest(receipt: &ChatReceipt, sender: Uuid, session: Uuid) -> Option<[u8; 32]> {
    if receipt.index < 0
        || receipt.content.len() > 1024
        || receipt.last_seen.len() > 20
        || receipt
            .last_seen
            .iter()
            .any(|signature| signature.len() != 256)
    {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(1i32.to_be_bytes());
    digest.update(sender.as_bytes());
    digest.update(session.as_bytes());
    digest.update(receipt.index.to_be_bytes());
    digest.update(receipt.salt.to_be_bytes());
    // Instant.getEpochSecond is floor division, including before the Unix epoch.
    digest.update(receipt.timestamp.div_euclid(1000).to_be_bytes());
    digest.update((receipt.content.len() as i32).to_be_bytes());
    digest.update(receipt.content.as_bytes());
    digest.update((receipt.last_seen.len() as i32).to_be_bytes());
    for signature in &receipt.last_seen {
        digest.update(signature);
    }
    Some(digest.finalize().into())
}
fn message_identity(receipt: &ChatReceipt, signed_digest: [u8; 32], signature: &[u8]) -> [u8; 32] {
    // Keep source record equality without cloning untrusted components/last-seen vectors per
    // message. Milliseconds, unsigned content and filtering are not all in the signed payload.
    let mut digest = Sha256::new();
    digest.update(signed_digest);
    digest.update(signature);
    digest.update(receipt.timestamp.to_be_bytes());
    digest.update(receipt.filter.to_be_bytes());
    digest.update((receipt.filter_bits.len() as u64).to_be_bytes());
    for bits in &receipt.filter_bits {
        digest.update(bits.to_be_bytes());
    }
    digest.update([u8::from(receipt.unsigned.is_some())]);
    if let Some(unsigned) = &receipt.unsigned {
        struct Writer<'a>(&'a mut Sha256);
        impl std::io::Write for Writer<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        // Value serialization into an infallible digest writer cannot fail.
        serde_json::to_writer(Writer(&mut digest), unsigned).expect("JSON Value digest");
    }
    digest.finalize().into()
}
