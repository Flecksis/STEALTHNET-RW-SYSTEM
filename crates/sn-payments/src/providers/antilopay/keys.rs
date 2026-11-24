use crate::error::{PaymentError, PaymentResult};
use base64::Engine;
use base64::alphabet;
use base64::engine::general_purpose::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use rsa::pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey};
use rsa::pkcs1v15::{Signature, SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::rand_core::OsRng;
use rsa::signature::{RandomizedSigner, SignatureEncoding, Verifier};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use std::ops::RangeInclusive;

const MODULUS_BITS: RangeInclusive<usize> = 496..=4096;
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

fn decoded(raw: &str) -> Option<Vec<u8>> {
    let raw = raw.replace("\\n", "").replace("\\r", "");
    let body = match raw.split_once("-----BEGIN ") {
        Some((_, rest)) => rest.split_once("-----")?.1,
        None => raw.as_str(),
    };
    let body = body.split_once("-----END ").map_or(body, |(body, _)| body);
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    BASE64.decode(compact.as_bytes()).ok()
}

fn fits(key: &impl PublicKeyParts) -> bool {
    MODULUS_BITS.contains(&key.n().bits())
}

fn private_key(raw: &str) -> Option<RsaPrivateKey> {
    let der = decoded(raw)?;
    RsaPrivateKey::from_pkcs8_der(&der)
        .ok()
        .or_else(|| RsaPrivateKey::from_pkcs1_der(&der).ok())
        .filter(fits)
}

fn public_key(raw: &str) -> Option<RsaPublicKey> {
    let der = decoded(raw)?;
    RsaPublicKey::from_public_key_der(&der)
        .ok()
        .or_else(|| RsaPublicKey::from_pkcs1_der(&der).ok())
        .filter(fits)
}

pub fn signature(secret_key: &str, body: &[u8]) -> Option<String> {
    signed(&SigningKey::<Sha256>::new(private_key(secret_key)?), body)
}

pub fn verify_signature(public: &str, body: &[u8], header: &str) -> bool {
    public_key(public).is_some_and(|key| genuine(&VerifyingKey::<Sha256>::new(key), body, header))
}

fn signed(key: &SigningKey<Sha256>, body: &[u8]) -> Option<String> {
    key.try_sign_with_rng(&mut OsRng, body)
        .ok()
        .map(|signature| STANDARD.encode(signature.to_bytes()))
}

fn genuine(key: &VerifyingKey<Sha256>, body: &[u8], header: &str) -> bool {
    let compact: String = header.chars().filter(|c| !c.is_whitespace()).collect();
    let Ok(raw) = BASE64.decode(compact.as_bytes()) else {
        return false;
    };
    Signature::try_from(raw.as_slice()).is_ok_and(|signature| key.verify(body, &signature).is_ok())
}

pub(super) struct Keys {
    signer: SigningKey<Sha256>,
    callback: VerifyingKey<Sha256>,
}

impl Keys {
    pub(super) fn parse(secret: &str, callback: &str) -> PaymentResult<Keys> {
        let signer = private_key(secret).ok_or_else(|| {
            PaymentError::NotConfigured(if public_key(secret).is_some() {
                "в поле «Секретный ключ» вставлен публичный ключ Antilopay".into()
            } else {
                "секретный ключ Antilopay не разобран: вставьте его целиком, он начинается с MII"
                    .into()
            })
        })?;
        let checker = public_key(callback).ok_or_else(|| {
            PaymentError::NotConfigured(if private_key(callback).is_some() {
                "в поле «Публичный ключ проекта» вставлен секретный ключ Antilopay".into()
            } else {
                "публичный ключ проекта Antilopay не разобран: вставьте его целиком".into()
            })
        })?;
        Ok(Keys {
            signer: SigningKey::new(signer),
            callback: VerifyingKey::new(checker),
        })
    }

    pub(super) fn sign(&self, body: &[u8]) -> PaymentResult<String> {
        signed(&self.signer, body).ok_or_else(|| {
            PaymentError::NotConfigured(
                "секретным ключом Antilopay не удалось подписать запрос".into(),
            )
        })
    }

    pub(super) fn genuine(&self, body: &[u8], header: &str) -> bool {
        genuine(&self.callback, body, header)
    }
}
