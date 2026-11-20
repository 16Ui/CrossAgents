use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, de::DeserializeOwned};
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct Envelope {
    format: String,
    nonce: String,
    data: String,
}

pub fn open<T: DeserializeOwned>(
    file: &str,
    key: &str,
    format: &str,
    limit: usize,
) -> Result<T, String> {
    let invalid = || "迁移文件或密钥无效".to_owned();
    if file.len() > limit || key.trim().len() != 43 {
        return Err(invalid());
    }
    let file: Envelope = serde_json::from_str(file).map_err(|_| invalid())?;
    if file.format != format {
        return Err(invalid());
    }
    let decoded = Zeroizing::new(URL_SAFE_NO_PAD.decode(key.trim()).map_err(|_| invalid())?);
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &decoded).map_err(|_| invalid())?);
    let nonce = URL_SAFE_NO_PAD.decode(file.nonce).map_err(|_| invalid())?;
    let nonce = Nonce::try_assume_unique_for_key(&nonce).map_err(|_| invalid())?;
    let mut bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(file.data).map_err(|_| invalid())?);
    let clear = key
        .open_in_place(nonce, Aad::from(format.as_bytes()), &mut bytes)
        .map_err(|_| invalid())?;
    serde_json::from_slice(clear).map_err(|_| invalid())
}
