//! Authored RSA fixture: Java-generated keys/signatures, not Mojang material. The pinned
//! Java 26.1.2 oracle verified this certificate/message before these consumer-boundary tests.
use super::*;
use crate::{
    entities::RemoteChatKey,
    protocol::chat_verification::ChatVerifier,
    ui::{ChatReceipt, ChatVerification},
};
const SERVICE: &str = "MIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEAmdpQDgmUSl8rPz3Ix0AAHU5J6vfzsp9KSg51HHWu54MEFR+Jb35WWUsu4JIJG7TZ7z4NUQ4bDBKDXcGqo7V6q+/KrFn/qwJkHgyDGm3Ag6p6zW/4tNJrSpR0/hFsw2JFyw/Bve1GkYh5jk3bwm+Vofv4iM3PpZVQQyzDKiKSshGUfEAuIjfQbqHyIuwMU3OtTYDpcpw+KK5azSEIEA9OQGSDE78roDdccGUd/soDaDv+8v0xPgQo+AlY+5csSwOxORRH+oycrqeiUQFpZhqAj1KcRPqS011TNsHeGMO+MKZWF9HfUk1N/lCzHn7Tws08U40vHWgHh7Ihid/NsLsELObUD46CeqFFL0q7X7zZhy7LfWo8/uQz04Cj7gJ/tNwiWtcULos9cA9+YPKfmQKtUBOh8GHFHX6JVZHOLwPAPAxP7BcQn5tj9XOrreuX5dqEOSNabz51hK0I28G+YIMKv+G/e4ovu8uEr4rZc36kKYzExBPI2kZNhOu74WHbyUYINC2DfruQCZQ6XQ/Uz5M/oL6BmLPZXO7ZcWEsdMj3ZqLicpbs3t+1X5rvKdbMySzDuWZarYy0kufP8ZAh6mC3sNwq1y8xNEcJ1JVV8vwMSAP1uqv+/xlkiG46xA+7Qd15AyTEoQ3K1hIZXWsYwGMOpe2i+kT/L9kW2V7TRZEYiD8CAwEAAQ==";
const PLAYER: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAsFzeNE+IIX7TBmmdfkMZUgJXowcuMuUtryrfDfaUAWNSou3BbzhZSR3Mtxm6EDYgD3hIr6zOa/nRjwHixWUSVlg0b+bxcRLs4CDmSYr0isTA2o0GTlPmeSuouGP9V2A53efbKqVKfto1UKvS3ZKbqPglExyNGotDazyX+75UsUCzUoVcNX7wL3ufqh0h6B9CtHWiAWa49kf7FWgP8jVP50eWa1FcA97btByFu4pst3LS/GMBlNcyWxcYkql7faniuQtab//MV7vRUWALSbwaiH5U40PNA/5QjaVkthKMrGGpcqlflIsqmPwXdShwawVlKs6qIXbHvRqzq8QTS//bFwIDAQAB";
const CERT: &str = "LhS+hbxprt7sXF8q84PXUD/bHXMuLfQABEBGmiPpJ31kHeAr4CiNW/GK0plpO06IdL/5vU9s7CDUM+HPvsm5HEJXmsnv7aihmgKpYNRL9GU31wi874tjE1KnrpjUIWtCyT79JLR/TgkRNMKBgGiZhdzAvY2xEmBpTsYzEZXy8orjtdQDtdyV+EIHoW+Kf11Vjk/Cy3hkppezrMhvOUOLShwbhmBgsqZj3/3L51wzECu4BtBNQvCYBMkMgVheb+iBD75uHVmAbr1eOTYGDwrBle+mJyP8MTudIY/r3OiJk6bWiskBbkUGwyY7+w6z6g8VkJhP1jQeuOqLKzFgw/Sl9Ae3JiBpRc3Yu3uW+SynEHpekI/EYKvotUXsCG7YiJMNzbndnyh4oCGI/3nUEnE90YfEf6WJ6d+J1/6/eNjwiF9WoCE1zk0LlZAf8euJSbFr1RbjKoqArfPLKABYLSZuNWGpbAe4qcHi7D4PxuTyciAO6qmJGhP0v2Q6Nr6i3EJRDO7JTCyVCUBW6ZUIY9R0UoVJU1F9xe4rzCPLpYrwe7UgU74zm5ndQtDp7ey2cJJEHzV8Z58DUB3OsLI0/snl8qT2BX9jixAiI+QZZLA68fGmnV+KsxVSpIStbJoAAV6auDKDt+F42MeNgUHqz+72tHiiAhiSWsFqxDlu7C3hf9k=";
const MESSAGE: &str = "aAxCt6nUuZz78g2JBB1PVjGW+5lZFmUR6F0GNLlzFk82n1auwSP56XkPEjnsSHN0gCD9f+GYwDb3vegmHAYeysnIRKXtO+EBgdGmOkU+vEU+CEGZZjqatzg4d/OkgYY0Bjaq5g9fvp8+oHmj8dIxxgrBAAT6YKxBBxM7GB6jWFIPIPNHUY3sMs22ptjmadf9zQh9uH9XYzMYdYzZwNfdSuDANXk3RynLS4GjjtsxVjSPqm3DhgKWdOnCiwC9xjyvdYH0uY+qs6ttHLI8oPJPylikURArq6wOx5662HgK4yGZSy0BP/7SB/s5JNvIPuJJJ+OTVCvduTQWcmT6zy21Wg==";
const GAP: &str = "Zn6uNLvvFNO9S90BVRCVcgsUfN8WWa8on0/ZE7GhsQFJ1XEqe3gon73bCiOQKRBbZ066mrtWcmFX3W5xAFb2J5LlE9psFSRcE3i/xhih/iVBcY0vTvw1mJBlMHVkn5OvqAr3CT01r8bkJS2UMMIYXlnzIUKPWJklGWzHZQAtQVmSU5pO7QfRoUM+Q+/BUmt+GwlygZnV9tVEhQYkRsQanZwZukjs7akF6cPv0+twQWi5Gc7Hm9tHOWZWGOjJE/jZEddNWoRyuQ8ggbzjUqNTYrPrKlketclDYi/Gjj95prnQy8Y+wcqpklUCCGprrRDJ5DwmVgZPGXTHPv8xSRqdcw==";
fn bytes(text: &str) -> Arc<[u8]> {
    Arc::from(STANDARD.decode(text).unwrap())
}
fn setup() -> (MojangServicesKeys, RemoteChatKey, ChatReceipt) {
    let response = serde_json::json!({"profilePropertyKeys":[{"publicKey":SERVICE}],"playerCertificateKeys":[{"publicKey":SERVICE}]});
    let services = MojangServicesKeys::default();
    *services.0.write() = Some(Arc::new(
        ServiceKeySet::parse(&serde_json::to_vec(&response).unwrap(), 1).unwrap(),
    ));
    let key = RemoteChatKey {
        session_id: "11223344-5566-7788-99aa-bbccddeeff00".into(),
        expires_at_millis: "1700000100000".into(),
        public_key_der: bytes(PLAYER),
        signature: bytes(CERT),
    };
    let receipt = ChatReceipt {
        sequence: 1,
        global_index: 0,
        index: 0,
        sender: "00112233-4455-6677-8899-aabbccddeeff".into(),
        signature: Some(bytes(MESSAGE)),
        content: Arc::from("Pinned Cyrillic Привет"),
        timestamp: 1700000000123,
        salt: -123456789,
        last_seen: vec![Arc::from([0u8; 256])],
        unsigned: None,
        filter: 0,
        filter_bits: Vec::new(),
    };
    (services, key, receipt)
}
#[test]
fn authenticated_message_chain_is_sticky_and_session_scoped() {
    let (services, mut key, receipt) = setup();
    let mut verifier = ChatVerifier::new(services);
    assert!(matches!(
        verifier.verify(&receipt, Some(&key), 1, receipt.timestamp),
        ChatVerification::Verified
    ));
    let mut duplicate = receipt.clone();
    duplicate.sequence = 2;
    duplicate.global_index = 1;
    assert!(matches!(
        verifier.verify(&duplicate, Some(&key), 1, receipt.timestamp),
        ChatVerification::Verified
    ));
    let mut gap = receipt.clone();
    gap.index = 3;
    gap.signature = Some(bytes(GAP));
    assert!(matches!(
        verifier.verify(&gap, Some(&key), 1, receipt.timestamp),
        ChatVerification::Verified
    ));
    assert!(matches!(
        verifier.verify(&receipt, Some(&key), 1, receipt.timestamp),
        ChatVerification::InvalidSignature
    ));
    assert!(matches!(
        verifier.verify(&gap, Some(&key), 1, receipt.timestamp),
        ChatVerification::InvalidSignature
    ));
    key.session_id = "11223344-5566-7788-99aa-bbccddeeff01".into();
    assert!(matches!(
        verifier.verify(&receipt, Some(&key), 1, receipt.timestamp),
        ChatVerification::InvalidSignature
    ));
    key.session_id = "11223344-5566-7788-99aa-bbccddeeff00".into();
    assert!(matches!(
        verifier.verify(&receipt, Some(&key), 1, receipt.timestamp),
        ChatVerification::Verified
    ));
}
#[test]
fn certificate_uuid_expiry_and_modified_duplicate_cannot_gain_verified_status() {
    let (services, key, receipt) = setup();
    let mut verifier = ChatVerifier::new(services.clone());
    let mut wrong = receipt.clone();
    wrong.sender = "00112233-4455-6677-8899-aabbccddeef0".into();
    assert!(matches!(
        verifier.verify(&wrong, Some(&key), 1, receipt.timestamp),
        ChatVerification::InvalidCertificate
    ));
    assert!(matches!(
        verifier.verify(
            &receipt,
            Some(&key),
            1,
            1700000100000 + 8 * 60 * 60 * 1000 + 1
        ),
        ChatVerification::ExpiredKey
    ));
    verifier.clear();
    assert!(matches!(
        verifier.verify(&receipt, Some(&key), 1, receipt.timestamp + 420000),
        ChatVerification::Verified
    ));
    assert!(matches!(
        verifier.verify(&receipt, Some(&key), 1, receipt.timestamp + 420001),
        ChatVerification::NotVerified
    ));
    let mut modified = receipt.clone();
    modified.unsigned = Some(serde_json::json!({"text":"substituted"}));
    assert!(matches!(
        verifier.verify(&modified, Some(&key), 1, receipt.timestamp),
        ChatVerification::InvalidSignature
    ));
    verifier.clear();
    let mut unsigned = receipt.clone();
    unsigned.signature = None;
    assert!(matches!(
        verifier.verify(&unsigned, Some(&key), 1, receipt.timestamp),
        ChatVerification::Unsigned
    ));
    assert!(matches!(
        verifier.verify(&receipt, Some(&key), 1, receipt.timestamp),
        ChatVerification::InvalidSignature
    ));
    assert!(matches!(
        ChatVerifier::new(MojangServicesKeys::default()).verify(
            &receipt,
            Some(&key),
            1,
            receipt.timestamp
        ),
        ChatVerification::MissingKey
    ));
}

#[test]
fn bad_signed_property_is_not_treated_as_unsigned() {
    let (services, _, _) = setup();
    let services = services.snapshot().unwrap();
    let mut property = ProfileProperty {
        name: "textures".into(),
        value: "eyJwcm9maWxlSWQiOiJkaWZmZXJlbnQiLCJ0ZXh0dXJlcyI6e319".into(),
        signature: Some("R+OWRjosfVwxk0Jd/otBfas1qrx5LKLo3GagljzGmIZK5dS/tfHzXEwflBEJw1pffDkMk5AQyeN7idG54UkajibErjIuqw5EAMVaMEMyfslSHlNvPdl2cebYxywX96Vkk80FVeC3Xy5n3QSzMMQvEiyUpeaShZwxBA+WIWZTUfRyK8bOk3zNuV33CzFqDUvc/nuUur3ksnXkslNLAUvsqAhE1W3hOlkd8kD4SMO9cwubFgGHr4L/b83Zk/ThJlX3+tKp4IKZfdeLjD2SPV+1zxU6PoWdJj8hc13YuuJN41M1bUf2+z4kbRqg92kZRHWkU5Vai8b9hITL3+fYlWtrGi8gExysiWcLktHIpO4LBKNjsEXLlOgQ8tcJG26UGTn6Dz9v6UumqJ3YA+hC3d4IdQAHmj7I2/1wdsNb1s37S/AFWOqgTteYuJN6O6FBzxcbiV1G2HtxRG+Uu+NCegNyy1y097P+Xu6ZzP6lC34Q3wwbivIY3DbGofqtYHKV0ApPk9RYrkKBIjgc8S2ncN2gylV1y1cqD4rYwsTG1duVl5tQm2bpyhkBOMMkueAqlvo1bTO/onVIJEZLdhJsTzCqUviroeKRQR5ksg512D86VAoV/OCiwxuePGmWC0qB2l+9R40ad3h+LWkNMaqWylYzxOTcmSt3d+bXVtuoPPFTYCs=".into()),
    };
    assert_eq!(services.classify_property(&property), TextureTrust::Signed);
    property.value.push(' ');
    assert_eq!(services.classify_property(&property), TextureTrust::Invalid);
    property.signature = Some("not-base64".into());
    assert_eq!(services.classify_property(&property), TextureTrust::Invalid);
    property.signature = None;
    assert_eq!(
        services.classify_property(&property),
        TextureTrust::Unsigned
    );
}
