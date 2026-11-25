//! A lab's keys: the key id, the public keys that a verifier trusts, and the key pair that a lab
//! signs with.
//!
//! Every key is Ed25519. A public key is its raw 32 bytes, and a receipt names it by its key id.
//! A private key is the 32-byte seed of RFC 8032, kept in a private key file.

use std::fmt;

use dyno_schema::canonical::sha256_hex;
use dyno_schema::receipt::{ED25519, PublicKey, PublicKeys};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::encoding::{base64url, bytes_of_base64url};
use crate::times;

/// How many bytes a raw Ed25519 public key has, and a private key's seed.
pub const KEY_BYTES: usize = 32;

/// How many bytes an Ed25519 signature has.
pub const SIGNATURE_BYTES: usize = 64;

/// How many hex characters a key id has.
pub const KEY_ID_LENGTH: usize = 16;

/// The key type of an Ed25519 key in a JSON Web Key (RFC 8037).
const JWK_KEY_TYPE: &str = "OKP";

/// The curve of an Ed25519 key in a JSON Web Key.
const JWK_CURVE: &str = "Ed25519";

/// The id of a public key: the first 16 hex characters of the SHA-256 of its raw 32 bytes.
pub fn key_id(public_key: &[u8; KEY_BYTES]) -> String {
    let mut id = sha256_hex(public_key);
    id.truncate(KEY_ID_LENGTH);
    id
}

/// Why a key cannot be used.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error(
        "{0:?} is not a public key: it must be the key's raw 32 bytes in base64url with no padding"
    )]
    NotAPublicKey(String),
    #[error(
        "the keys document gives key {stated} a public key whose key id is {computed}: the id \
         must be the first 16 hex characters of the SHA-256 of the raw public key"
    )]
    WrongKeyId { stated: String, computed: String },
    #[error(
        "key {key_id} in the keys document has a {field} that is not an RFC 3339 time: {text:?}"
    )]
    NotATime {
        key_id: String,
        field: &'static str,
        text: String,
    },
    #[error("it is not a private key file that dyno receipt keygen wrote: {0}")]
    NotAPrivateKey(&'static str),
    #[error("the system gave no random bytes for a new key")]
    NoRandomBytes,
}

/// A public key that the person who verifies trusts to be the lab's: one they gave themselves, or
/// one from the lab's keys document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedKey {
    key_id: String,
    public_key: [u8; KEY_BYTES],
    created_at: Option<String>,
    retired_at: Option<Retired>,
}

/// When a lab retired a key.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Retired {
    /// The time as the keys document states it.
    stated: String,
    time: OffsetDateTime,
}

impl TrustedKey {
    /// A key given as its raw 32 bytes in base64url with no padding. It has no times, so it is
    /// never retired.
    pub fn from_base64url(text: &str) -> Result<TrustedKey, KeyError> {
        let public_key = bytes_of_base64url(text.trim())
            .ok_or_else(|| KeyError::NotAPublicKey(text.to_owned()))?;
        Ok(TrustedKey {
            key_id: key_id(&public_key),
            public_key,
            created_at: None,
            retired_at: None,
        })
    }

    /// A key of a keys document. The key id it states must be its public key's, so that nothing
    /// later can take one key for another.
    fn from_document(entry: &PublicKey) -> Result<TrustedKey, KeyError> {
        let key = TrustedKey::from_base64url(&entry.public_key)?;
        if key.key_id != entry.key_id {
            return Err(KeyError::WrongKeyId {
                stated: entry.key_id.clone(),
                computed: key.key_id,
            });
        }
        let time = |field: &'static str, text: &str| {
            times::parse(text).ok_or_else(|| KeyError::NotATime {
                key_id: entry.key_id.clone(),
                field,
                text: text.to_owned(),
            })
        };
        time("created_at", &entry.created_at)?;
        let retired_at = match &entry.retired_at {
            Some(stated) => Some(Retired {
                stated: stated.clone(),
                time: time("retired_at", stated)?,
            }),
            None => None,
        };
        Ok(TrustedKey {
            created_at: Some(entry.created_at.clone()),
            retired_at,
            ..key
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// When the lab made the key, as its keys document states it. None for a key given alone.
    pub fn created_at(&self) -> Option<&str> {
        self.created_at.as_deref()
    }

    /// When the lab retired the key, as its keys document states it. None when it has not.
    pub fn retired_at(&self) -> Option<&str> {
        self.retired_at
            .as_ref()
            .map(|retired| retired.stated.as_str())
    }

    /// Whether the lab had retired the key at `time`. A key is retired from its `retired_at` on.
    pub(crate) fn was_retired_at(&self, time: OffsetDateTime) -> bool {
        self.retired_at
            .as_ref()
            .is_some_and(|retired| retired.time <= time)
    }

    /// Whether `signature` is this key's Ed25519 signature of `message`.
    pub fn signed(&self, message: &[u8], signature: &[u8; SIGNATURE_BYTES]) -> bool {
        UnparsedPublicKey::new(&signature::ED25519, &self.public_key)
            .verify(message, signature)
            .is_ok()
    }
}

/// The keys of a lab's keys document that this dyno can verify with. A key of an algorithm that
/// this dyno does not know is left out, so a lab can add one later. A key that is not what its
/// entry says makes the whole document an error.
pub fn trusted_keys(document: &PublicKeys) -> Result<Vec<TrustedKey>, KeyError> {
    document
        .keys
        .iter()
        .filter(|entry| entry.algorithm == ED25519)
        .map(TrustedKey::from_document)
        .collect()
}

/// A lab's key pair: it signs receipts.
pub struct SigningKey {
    seed: [u8; KEY_BYTES],
    pair: Ed25519KeyPair,
}

/// The private key file: a JSON Web Key for Ed25519 (RFC 8037), so other tools read it too.
///
/// `d` is the private key, so the file is a secret. It has no `Debug`, so that no log line can
/// print it.
#[derive(Serialize, Deserialize)]
pub struct PrivateKeyFile {
    /// The key type: `OKP`.
    pub kty: String,
    /// The curve: `Ed25519`.
    pub crv: String,
    /// The key id, as receipts name the key. A file from another tool may leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    /// The public key: its raw 32 bytes in base64url with no padding.
    pub x: String,
    /// The private key: the 32-byte seed of RFC 8032, in base64url with no padding.
    pub d: String,
}

impl SigningKey {
    /// A new key pair, from the system's random bytes.
    pub fn generate() -> Result<SigningKey, KeyError> {
        let mut seed = [0; KEY_BYTES];
        SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| KeyError::NoRandomBytes)?;
        Ok(SigningKey::from_seed(seed))
    }

    /// The key pair of a private key: the 32-byte seed of RFC 8032.
    pub fn from_seed(seed: [u8; KEY_BYTES]) -> SigningKey {
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed)
            .expect("every seed of 32 bytes is an Ed25519 private key");
        SigningKey { seed, pair }
    }

    /// The key pair of a private key file. The file's public key must be the private key's, and
    /// so must its key id when it has one: a file that was edited, or put together from two keys,
    /// is refused.
    pub fn from_file(file: &PrivateKeyFile) -> Result<SigningKey, KeyError> {
        if file.kty != JWK_KEY_TYPE || file.crv != JWK_CURVE {
            return Err(KeyError::NotAPrivateKey(
                "its kty must be OKP and its crv Ed25519",
            ));
        }
        let seed = bytes_of_base64url(&file.d).ok_or(KeyError::NotAPrivateKey(
            "its d must be 32 bytes in base64url with no padding",
        ))?;
        let key = SigningKey::from_seed(seed);
        if bytes_of_base64url(&file.x) != Some(key.public_key()) {
            return Err(KeyError::NotAPrivateKey(
                "its x is not the public key of its d",
            ));
        }
        if file.kid.as_ref().is_some_and(|kid| *kid != key.key_id()) {
            return Err(KeyError::NotAPrivateKey(
                "its kid is not the key id of its public key",
            ));
        }
        Ok(key)
    }

    /// The private key file of this key pair.
    pub fn to_file(&self) -> PrivateKeyFile {
        PrivateKeyFile {
            kty: JWK_KEY_TYPE.to_owned(),
            crv: JWK_CURVE.to_owned(),
            kid: Some(self.key_id()),
            x: base64url(&self.public_key()),
            d: base64url(&self.seed),
        }
    }

    /// The raw 32 bytes of the public key.
    pub fn public_key(&self) -> [u8; KEY_BYTES] {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .expect("an Ed25519 public key has 32 bytes")
    }

    /// The id that receipts name this key by.
    pub fn key_id(&self) -> String {
        key_id(&self.public_key())
    }

    /// This key's entry in the lab's keys document, as a key made at `created_at` that is not
    /// retired.
    pub fn public_entry(&self, created_at: &str) -> PublicKey {
        PublicKey {
            key_id: self.key_id(),
            algorithm: ED25519.to_owned(),
            public_key: base64url(&self.public_key()),
            created_at: created_at.to_owned(),
            retired_at: None,
        }
    }

    /// The Ed25519 signature of `message`. The same key and message always give the same
    /// signature.
    pub fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_BYTES] {
        self.pair
            .sign(message)
            .as_ref()
            .try_into()
            .expect("an Ed25519 signature has 64 bytes")
    }
}

/// Only the key id: the private key must not reach a log.
impl fmt::Debug for SigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SigningKey({})", self.key_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first and second test keys of RFC 8032, section 7.1: the seed, the public key, the
    /// message and its signature, in hex.
    const RFC_8032: [(&str, &str, &str, &str); 2] = [
        (
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "",
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
             5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        ),
        (
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
            "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            "72",
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da\
             085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        ),
    ];

    /// The first RFC 8032 public key, in base64url, and its key id.
    const PUBLIC_KEY: &str = "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo";
    const KEY_ID: &str = "21fe31dfa154a261";

    fn bytes<const N: usize>(hex: &str) -> [u8; N] {
        let pairs: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect();
        pairs.try_into().unwrap()
    }

    fn rfc_key() -> SigningKey {
        SigningKey::from_seed(bytes(RFC_8032[0].0))
    }

    fn entry(key: &SigningKey, retired_at: Option<&str>) -> PublicKey {
        PublicKey {
            retired_at: retired_at.map(str::to_owned),
            ..key.public_entry("2026-11-01T00:00:00Z")
        }
    }

    #[test]
    fn the_rfc_8032_keys_give_the_rfc_s_public_keys_and_signatures() {
        for (seed, public_key, message, signature) in RFC_8032 {
            let key = SigningKey::from_seed(bytes(seed));
            let message: Vec<u8> = if message.is_empty() {
                Vec::new()
            } else {
                bytes::<1>(message).to_vec()
            };
            assert_eq!(key.public_key(), bytes::<32>(public_key));
            assert_eq!(key.sign(&message), bytes::<64>(signature));
        }
    }

    #[test]
    fn a_key_id_is_the_first_16_hex_characters_of_the_sha_256_of_the_public_key() {
        let key = rfc_key();
        let hash = sha256_hex(&key.public_key());
        assert_eq!(key.key_id(), hash[..16]);
        assert_eq!(key.key_id(), KEY_ID);
    }

    #[test]
    fn a_key_verifies_its_own_signature_and_no_other_message_or_key() {
        let key = rfc_key();
        let trusted = TrustedKey::from_base64url(PUBLIC_KEY).unwrap();
        assert_eq!(trusted.key_id(), KEY_ID);
        let signature = key.sign(b"a receipt");
        assert!(trusted.signed(b"a receipt", &signature));
        assert!(!trusted.signed(b"a receipt.", &signature));
        let mut changed = signature;
        changed[0] ^= 1;
        assert!(!trusted.signed(b"a receipt", &changed));
        let other = SigningKey::from_seed(bytes(RFC_8032[1].0));
        assert!(!trusted.signed(b"a receipt", &other.sign(b"a receipt")));
    }

    #[test]
    fn a_new_key_is_not_the_one_before_it() {
        let one = SigningKey::generate().unwrap();
        let other = SigningKey::generate().unwrap();
        assert_ne!(one.public_key(), other.public_key());
        assert_eq!(one.key_id().len(), KEY_ID_LENGTH);
    }

    #[test]
    fn text_that_is_not_32_bytes_of_base64url_is_not_a_public_key() {
        for text in [
            "",
            "AAAA",
            "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
            "not base64!",
        ] {
            assert_eq!(
                TrustedKey::from_base64url(text),
                Err(KeyError::NotAPublicKey(text.to_owned())),
            );
        }
    }

    #[test]
    fn a_keys_document_gives_each_ed25519_key_with_its_times() {
        let key = rfc_key();
        let other = SigningKey::from_seed(bytes(RFC_8032[1].0));
        let document = PublicKeys {
            keys: vec![
                entry(&key, Some("2026-12-01T00:00:00.000+00:00")),
                entry(&other, None),
            ],
        };
        let keys = trusted_keys(&document).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].key_id(), KEY_ID);
        assert_eq!(keys[0].created_at(), Some("2026-11-01T00:00:00Z"));
        assert_eq!(keys[0].retired_at(), Some("2026-12-01T00:00:00.000+00:00"));
        assert_eq!(keys[1].retired_at(), None);
    }

    #[test]
    fn a_key_is_retired_from_its_retired_at_on() {
        let retired =
            TrustedKey::from_document(&entry(&rfc_key(), Some("2026-12-01T00:00:00Z"))).unwrap();
        let at = |text| times::parse(text).unwrap();
        assert!(!retired.was_retired_at(at("2026-11-30T23:59:59Z")));
        assert!(retired.was_retired_at(at("2026-12-01T00:00:00Z")));
        assert!(retired.was_retired_at(at("2026-12-01T00:00:01Z")));
        let in_use = TrustedKey::from_base64url(PUBLIC_KEY).unwrap();
        assert!(!in_use.was_retired_at(at("2999-01-01T00:00:00Z")));
    }

    #[test]
    fn a_key_of_an_algorithm_this_dyno_does_not_know_is_left_out() {
        let mut newer = entry(&rfc_key(), None);
        newer.algorithm = "ml-dsa-65".to_owned();
        newer.public_key = "a longer key".to_owned();
        let document = PublicKeys { keys: vec![newer] };
        assert_eq!(trusted_keys(&document), Ok(Vec::new()));
    }

    #[test]
    fn a_keys_document_with_a_key_id_that_is_not_its_key_s_is_an_error() {
        let mut wrong = entry(&rfc_key(), None);
        wrong.key_id = "0000000000000000".to_owned();
        let error = trusted_keys(&PublicKeys { keys: vec![wrong] }).unwrap_err();
        assert_eq!(
            error,
            KeyError::WrongKeyId {
                stated: "0000000000000000".to_owned(),
                computed: KEY_ID.to_owned(),
            }
        );
    }

    #[test]
    fn a_keys_document_with_a_bad_key_or_a_bad_time_is_an_error() {
        let key = rfc_key();
        let mut bad_key = entry(&key, None);
        bad_key.public_key = "AAAA".to_owned();
        let mut bad_created = entry(&key, None);
        bad_created.created_at = "last week".to_owned();
        let bad_retired = entry(&key, Some("soon"));
        let error = |entry| trusted_keys(&PublicKeys { keys: vec![entry] }).unwrap_err();
        assert_eq!(error(bad_key), KeyError::NotAPublicKey("AAAA".to_owned()));
        let message = error(bad_created).to_string();
        assert!(
            message.contains("created_at") && message.contains("last week"),
            "{message}"
        );
        let message = error(bad_retired).to_string();
        assert!(
            message.contains("retired_at") && message.contains("soon"),
            "{message}"
        );
    }

    #[test]
    fn a_private_key_file_is_a_json_web_key_that_gives_the_same_key_back() {
        let key = rfc_key();
        let file = key.to_file();
        let json = serde_json::to_value(&file).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kty": "OKP",
                "crv": "Ed25519",
                "kid": KEY_ID,
                "x": PUBLIC_KEY,
                "d": "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A",
            })
        );
        let read: PrivateKeyFile = serde_json::from_value(json).unwrap();
        let again = SigningKey::from_file(&read).unwrap();
        assert_eq!(again.public_key(), key.public_key());
        assert_eq!(again.sign(b"x"), key.sign(b"x"));
    }

    #[test]
    fn a_private_key_file_from_another_tool_needs_no_key_id() {
        let mut file = rfc_key().to_file();
        file.kid = None;
        assert_eq!(SigningKey::from_file(&file).unwrap().key_id(), KEY_ID);
        assert!(!serde_json::to_string(&file).unwrap().contains("kid"));
    }

    #[test]
    fn a_private_key_file_that_is_not_one_key_is_refused_and_says_why() {
        let good = || rfc_key().to_file();
        let other = SigningKey::from_seed(bytes(RFC_8032[1].0)).to_file();
        let cases = [
            (
                PrivateKeyFile {
                    kty: "RSA".to_owned(),
                    ..good()
                },
                "kty must be OKP",
            ),
            (
                PrivateKeyFile {
                    crv: "X25519".to_owned(),
                    ..good()
                },
                "crv Ed25519",
            ),
            (
                PrivateKeyFile {
                    d: "AAAA".to_owned(),
                    ..good()
                },
                "d must be 32 bytes",
            ),
            (
                PrivateKeyFile {
                    x: other.x,
                    ..good()
                },
                "x is not the public key",
            ),
            (
                PrivateKeyFile {
                    kid: other.kid,
                    ..good()
                },
                "kid is not the key id",
            ),
        ];
        for (file, why) in cases {
            let error = SigningKey::from_file(&file).unwrap_err().to_string();
            assert!(error.contains(why), "{error}");
        }
    }

    #[test]
    fn a_signing_key_never_prints_its_private_key() {
        let shown = format!("{:?}", rfc_key());
        assert_eq!(shown, format!("SigningKey({KEY_ID})"));
    }
}
