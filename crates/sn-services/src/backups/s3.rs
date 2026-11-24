use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

const SINGLE_MAX: u64 = 32 * 1024 * 1024;
const PART: u64 = 16 * 1024 * 1024;
const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Place {
    pub endpoint: String,
    #[serde(default)]
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
    pub access_key: String,
    #[serde(default)]
    pub virtual_host: bool,
}

impl S3Place {
    pub fn clean(mut self) -> Result<Self, String> {
        self.endpoint = self.endpoint.trim().trim_end_matches('/').to_string();
        self.region = self.region.trim().to_string();
        if self.region.is_empty() {
            self.region = "us-east-1".into();
        }
        self.bucket = self.bucket.trim().to_string();
        self.access_key = self.access_key.trim().to_string();
        let prefix = self.prefix.trim().trim_matches('/').to_string();
        self.prefix = if prefix.is_empty() {
            prefix
        } else {
            format!("{prefix}/")
        };
        let url = reqwest::Url::parse(&self.endpoint)
            .map_err(|_| "адрес хранилища должен быть вида https://s3.example.com".to_string())?;
        if url.scheme() != "https" && url.scheme() != "http" {
            return Err("адрес хранилища должен начинаться с https://".into());
        }
        if url.host_str().is_none() || url.path() != "/" || url.query().is_some() {
            return Err(
                "укажите только адрес хранилища без пути, например https://s3.example.com".into(),
            );
        }
        if self.bucket.is_empty()
            || self.bucket.len() > 63
            || !self
                .bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        {
            return Err("имя бакета: строчные латинские буквы, цифры, точки и дефисы".into());
        }
        if self.access_key.is_empty() {
            return Err("укажите ключ доступа".into());
        }
        if self.region.len() > 40
            || !self
                .region
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("регион: латинские буквы, цифры и дефисы, например eu-central-1".into());
        }
        if self.prefix.len() > 200
            || self
                .prefix
                .bytes()
                .any(|b| b.is_ascii_control() || b == b'\\' || b == b'?' || b == b'#')
        {
            return Err("папка в бакете: латинские буквы, цифры, дефисы и косые черты".into());
        }
        Ok(self)
    }

    pub fn key_for(&self, file: &str) -> String {
        format!("{}{file}", self.prefix)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Error {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl S3Error {
    fn transport(err: reqwest::Error) -> Self {
        S3Error {
            status: 0,
            code: "transport".into(),
            message: err.without_url().to_string(),
        }
    }

    pub fn readable(&self) -> String {
        match self.code.as_str() {
            "transport" => "хранилище не отвечает: проверьте адрес".into(),
            "AccessDenied" | "AllAccessDisabled" => {
                "нет доступа: у ключа нет права записывать в этот бакет".into()
            }
            "NoSuchBucket" => "такого бакета нет: проверьте имя".into(),
            "InvalidAccessKeyId" => "ключ доступа не найден в хранилище".into(),
            "SignatureDoesNotMatch" => "секретный ключ не подходит к ключу доступа".into(),
            "RequestTimeTooSkewed" => "часы сервера сильно расходятся с хранилищем".into(),
            "AuthorizationHeaderMalformed"
            | "PermanentRedirect"
            | "IllegalLocationConstraintException" => "регион указан неверно".into(),
            "EntityTooLarge" => "хранилище не принимает файлы такого размера".into(),
            "QuotaExceeded" | "StorageQuotaExceeded" => "в хранилище кончилось место".into(),
            _ if self.status >= 500 => format!("хранилище не ответило: ошибка {}", self.status),
            _ if self.message.is_empty() => {
                format!("хранилище ответило {} {}", self.status, self.code)
            }
            _ => format!("хранилище ответило: {}", self.message),
        }
    }
}

fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

fn encode(value: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let plain = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if plain || (keep_slash && byte == b'/') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("ключ любой длины");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub struct Signed {
    pub authorization: String,
    pub amz_date: String,
    pub content_sha256: String,
}

#[allow(clippy::too_many_arguments)]
pub fn sign(
    access_key: &str,
    secret_key: &str,
    region: &str,
    method: &str,
    host: &str,
    canonical_uri: &str,
    query: &[(String, String)],
    extra_headers: &[(String, String)],
    payload_sha256: &str,
    now: DateTime<Utc>,
) -> Signed {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let mut query: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (encode(k, false), encode(v, false)))
        .collect();
    query.sort();
    let canonical_query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), host.to_string()),
        ("x-amz-content-sha256".into(), payload_sha256.to_string()),
        ("x-amz-date".into(), amz_date.clone()),
    ];
    headers.extend(
        extra_headers
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())),
    );
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_sha256}"
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let key = hmac(format!("AWS4{secret_key}").as_bytes(), &date);
    let key = hmac(&key, region);
    let key = hmac(&key, "s3");
    let key = hmac(&key, "aws4_request");
    let signature = hex::encode(hmac(&key, &string_to_sign));
    Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
        ),
        amz_date,
        content_sha256: payload_sha256.to_string(),
    }
}

pub struct Client {
    http: reqwest::Client,
    place: S3Place,
    secret: String,
}

struct Target {
    url: reqwest::Url,
    host: String,
    canonical_uri: String,
}

impl Client {
    pub fn new(place: S3Place, secret: String) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(900))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Client {
            http,
            place,
            secret,
        })
    }

    fn target(&self, key: &str, query: &[(String, String)]) -> Result<Target, S3Error> {
        let base = reqwest::Url::parse(&self.place.endpoint).map_err(|_| S3Error {
            status: 0,
            code: "transport".into(),
            message: "неверный адрес".into(),
        })?;
        let scheme = base.scheme();
        let base_host = base.host_str().unwrap_or_default();
        let port = base.port().map(|p| format!(":{p}")).unwrap_or_default();
        let key_path = encode(key, true);
        let (host, canonical_uri) = if self.place.virtual_host {
            (
                format!("{}.{base_host}{port}", self.place.bucket),
                format!("/{key_path}"),
            )
        } else {
            (
                format!("{base_host}{port}"),
                format!("/{}/{key_path}", encode(&self.place.bucket, false)),
            )
        };
        let mut url = format!("{scheme}://{host}{canonical_uri}");
        if !query.is_empty() {
            let mut sorted: Vec<(String, String)> = query
                .iter()
                .map(|(k, v)| (encode(k, false), encode(v, false)))
                .collect();
            sorted.sort();
            url.push('?');
            url.push_str(
                &sorted
                    .iter()
                    .map(|(k, v)| {
                        if v.is_empty() {
                            k.clone()
                        } else {
                            format!("{k}={v}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        let url = reqwest::Url::parse(&url).map_err(|_| S3Error {
            status: 0,
            code: "transport".into(),
            message: "неверный адрес".into(),
        })?;
        Ok(Target {
            url,
            host,
            canonical_uri,
        })
    }

    async fn send(
        &self,
        method: reqwest::Method,
        key: &str,
        query: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<(reqwest::header::HeaderMap, String), S3Error> {
        let target = self.target(key, query)?;
        let payload = if body.is_empty() {
            EMPTY_SHA.to_string()
        } else {
            hex::encode(Sha256::digest(&body))
        };
        let signed = sign(
            &self.place.access_key,
            &self.secret,
            &self.place.region,
            method.as_str(),
            &target.host,
            &target.canonical_uri,
            query,
            &[],
            &payload,
            Utc::now(),
        );
        let response = self
            .http
            .request(method, target.url)
            .header("authorization", signed.authorization)
            .header("x-amz-date", signed.amz_date)
            .header("x-amz-content-sha256", signed.content_sha256)
            .body(body)
            .send()
            .await
            .map_err(S3Error::transport)?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().await.unwrap_or_default();
        if (200..300).contains(&status) && !text.contains("<Error>") {
            return Ok((headers, text));
        }
        Err(S3Error {
            status,
            code: tag(&text, "Code").unwrap_or_else(|| status.to_string()),
            message: tag(&text, "Message").unwrap_or_default(),
        })
    }

    pub async fn put(&self, key: &str, body: Vec<u8>) -> Result<(), S3Error> {
        self.send(reqwest::Method::PUT, key, &[], body)
            .await
            .map(|_| ())
    }

    pub async fn delete(&self, key: &str) -> Result<(), S3Error> {
        self.send(reqwest::Method::DELETE, key, &[], Vec::new())
            .await
            .map(|_| ())
    }

    pub async fn upload_file(&self, key: &str, path: &Path, size: u64) -> Result<(), S3Error> {
        if size <= SINGLE_MAX {
            let body = read_range(path, 0, size).await?;
            return self.put(key, body).await;
        }
        let (_, text) = self
            .send(
                reqwest::Method::POST,
                key,
                &[("uploads".into(), String::new())],
                Vec::new(),
            )
            .await?;
        let upload = tag(&text, "UploadId").ok_or_else(|| S3Error {
            status: 200,
            code: "NoUploadId".into(),
            message: "хранилище не начало загрузку частями".into(),
        })?;
        let result = self.parts(key, &upload, path, size).await;
        match result {
            Ok(etags) => {
                let mut body = String::from("<CompleteMultipartUpload>");
                for (index, etag) in etags.iter().enumerate() {
                    body.push_str(&format!(
                        "<Part><PartNumber>{}</PartNumber><ETag>{etag}</ETag></Part>",
                        index + 1
                    ));
                }
                body.push_str("</CompleteMultipartUpload>");
                let done = self
                    .send(
                        reqwest::Method::POST,
                        key,
                        &[("uploadId".into(), upload.clone())],
                        body.into_bytes(),
                    )
                    .await;
                if done.is_err() {
                    self.abort(key, &upload).await;
                }
                done.map(|_| ())
            }
            Err(err) => {
                self.abort(key, &upload).await;
                Err(err)
            }
        }
    }

    async fn parts(
        &self,
        key: &str,
        upload: &str,
        path: &Path,
        size: u64,
    ) -> Result<Vec<String>, S3Error> {
        let mut etags = Vec::new();
        let mut offset = 0u64;
        let mut number = 1u32;
        while offset < size {
            let len = PART.min(size - offset);
            let body = read_range(path, offset, len).await?;
            let (headers, _) = self
                .send(
                    reqwest::Method::PUT,
                    key,
                    &[
                        ("partNumber".into(), number.to_string()),
                        ("uploadId".into(), upload.to_string()),
                    ],
                    body,
                )
                .await?;
            let etag = headers
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .ok_or_else(|| S3Error {
                    status: 200,
                    code: "NoETag".into(),
                    message: "хранилище не вернуло отметку части".into(),
                })?;
            etags.push(etag);
            offset += len;
            number += 1;
        }
        Ok(etags)
    }

    async fn abort(&self, key: &str, upload: &str) {
        let _ = self
            .send(
                reqwest::Method::DELETE,
                key,
                &[("uploadId".into(), upload.to_string())],
                Vec::new(),
            )
            .await;
    }
}

async fn read_range(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, S3Error> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(&path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut out = Vec::with_capacity(len as usize);
        file.take(len).read_to_end(&mut out)?;
        Ok(out)
    })
    .await
    .map_err(|e| S3Error {
        status: 0,
        code: "file".into(),
        message: e.to_string(),
    })?
    .map_err(|e| S3Error {
        status: 0,
        code: "file".into(),
        message: format!("файл копии не читается: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn хранилище_не_уводит_копию_по_редиректу() {
        let elsewhere = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .mount(&elsewhere)
            .await;
        let storage = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/b/copy.tar.gz", elsewhere.uri())),
            )
            .mount(&storage)
            .await;
        let client = Client::new(
            S3Place {
                endpoint: storage.uri(),
                region: "us-east-1".into(),
                bucket: "b".into(),
                prefix: String::new(),
                access_key: "AKIAIOSFODNN7EXAMPLE".into(),
                virtual_host: false,
            },
            "secret".into(),
        )
        .unwrap();
        let err = client
            .put("copy.tar.gz", b"backup".to_vec())
            .await
            .unwrap_err();
        assert_eq!(err.status, 307);
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn подпись_совпадает_с_примером_из_документации_aws() {
        let now = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let signed = sign(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "GET",
            "examplebucket.s3.amazonaws.com",
            "/test.txt",
            &[],
            &[("range".into(), "bytes=0-9".into())],
            EMPTY_SHA,
            now,
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn адрес_и_бакет_проверяются() {
        let place = S3Place {
            endpoint: " https://s3.example.com/ ".into(),
            region: String::new(),
            bucket: "sn-backups".into(),
            prefix: "/panel/".into(),
            access_key: "AK".into(),
            virtual_host: false,
        }
        .clean()
        .unwrap();
        assert_eq!(place.endpoint, "https://s3.example.com");
        assert_eq!(place.region, "us-east-1");
        assert_eq!(place.key_for("a.tar.gz"), "panel/a.tar.gz");
        let bad = |f: fn(&mut S3Place)| {
            let mut p = place.clone();
            f(&mut p);
            p.clean().is_err()
        };
        assert!(bad(|p| p.endpoint = "s3.example.com".into()));
        assert!(bad(|p| p.endpoint = "https://s3.example.com/bucket".into()));
        assert!(bad(|p| p.bucket = "Big_Bucket".into()));
        assert!(bad(|p| p.access_key = " ".into()));
        assert!(bad(|p| p.region = "eu central".into()));
    }

    #[test]
    fn ключи_и_запросы_кодируются_как_в_s3() {
        assert_eq!(encode("panel/a b+c.tar.gz", true), "panel/a%20b%2Bc.tar.gz");
        assert_eq!(encode("a/b", false), "a%2Fb");
        assert_eq!(
            tag("<Error><Code>AccessDenied</Code></Error>", "Code").as_deref(),
            Some("AccessDenied")
        );
        assert!(
            S3Error {
                status: 403,
                code: "AccessDenied".into(),
                message: String::new()
            }
            .readable()
            .contains("нет доступа")
        );
    }
}
