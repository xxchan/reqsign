//! Azure Storage Singer

use std::fmt::Debug;
use std::fmt::Write;
use std::time::Duration;

use anyhow::anyhow;
use anyhow::Result;
use http::header::*;
use log::debug;

use super::super::constants::*;
use super::credential::Credential;
use crate::ctx::SigningContext;
use crate::ctx::SigningMethod;
use crate::hash::base64_decode;
use crate::hash::base64_hmac_sha256;
use crate::request::SignableRequest;
use crate::time;
use crate::time::format_http_date;
use crate::time::DateTime;

/// Singer that implement Azure Storage Shared Key Authorization.
///
/// - [Authorize with Shared Key](https://docs.microsoft.com/en-us/rest/api/storageservices/authorize-with-shared-key)
#[derive(Debug, Default)]
pub struct Signer {
    /// whether to omit service version or not
    omit_service_version: bool,
    time: Option<DateTime>,
}

impl Signer {
    /// Create a signer.
    pub fn new() -> Self {
        Self::default()
    }

    /// set the signer to omitting service version
    pub fn omit_service_version(mut self) -> Self {
        self.omit_service_version = true;
        self
    }

    /// Specify the signing time.
    ///
    /// # Note
    ///
    /// We should always take current time to sign requests.
    /// Only use this function for testing.
    #[cfg(test)]
    pub fn time(&mut self, time: DateTime) -> &mut Self {
        self.time = Some(time);
        self
    }

    fn build(
        &self,
        req: &mut impl SignableRequest,
        method: SigningMethod,
        cred: &Credential,
    ) -> Result<SigningContext> {
        let mut ctx = req.build()?;

        match cred {
            Credential::SharedAccessSignature(token) => {
                ctx.query_append(token);
                return Ok(ctx);
            }
            Credential::SharedKey(ak, sk) => match method {
                SigningMethod::Query(_) => {
                    return Err(anyhow!("SAS token is required for query signing"));
                }
                SigningMethod::Header => {
                    let now = self.time.unwrap_or_else(time::now);
                    let string_to_sign =
                        string_to_sign(&mut ctx, ak, now, self.omit_service_version)?;
                    let signature =
                        base64_hmac_sha256(&base64_decode(sk), string_to_sign.as_bytes());

                    ctx.headers.insert(AUTHORIZATION, {
                        let mut value: HeaderValue =
                            format!("SharedKey {ak}:{signature}").parse()?;
                        value.set_sensitive(true);

                        value
                    });
                }
            },
        }

        Ok(ctx)
    }

    /// Signing request.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use anyhow::Result;
    /// use reqsign::AzureStorageSigner;
    /// use reqwest::Client;
    /// use reqwest::Request;
    /// use reqwest::Url;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     // Signer will load region and credentials from environment by default.
    ///     let signer = AzureStorageSigner::builder()
    ///         .account_name("account_name")
    ///         .account_key("YWNjb3VudF9rZXkK")
    ///         .build()?;
    ///     // Construct request
    ///     let url = Url::parse("https://test.blob.core.windows.net/testbucket/testblob")?;
    ///     let mut req = reqwest::Request::new(http::Method::GET, url);
    ///     // Signing request with Signer
    ///     signer.sign(&mut req)?;
    ///     // Sending already signed request.
    ///     let resp = Client::new().execute(req).await?;
    ///     println!("resp got status: {}", resp.status());
    ///     Ok(())
    /// }
    /// ```
    pub fn sign(&self, req: &mut impl SignableRequest, cred: &Credential) -> Result<()> {
        let ctx = self.build(req, SigningMethod::Header, cred)?;
        req.apply(ctx)
    }

    /// Signing request with query.
    pub fn sign_query(&self, req: &mut impl SignableRequest, cred: &Credential) -> Result<()> {
        let ctx = self.build(req, SigningMethod::Query(Duration::from_secs(1)), cred)?;
        req.apply(ctx)
    }
}

/// Construct string to sign
///
/// ## Format
///
/// ```text
/// VERB + "\n" +
/// Content-Encoding + "\n" +
/// Content-Language + "\n" +
/// Content-Length + "\n" +
/// Content-MD5 + "\n" +
/// Content-Type + "\n" +
/// Date + "\n" +
/// If-Modified-Since + "\n" +
/// If-Match + "\n" +
/// If-None-Match + "\n" +
/// If-Unmodified-Since + "\n" +
/// Range + "\n" +
/// CanonicalizedHeaders +
/// CanonicalizedResource;
/// ```
/// ## Note
/// For sub-requests of batch API, requests should be signed without `x-ms-version` header.
/// Set the `omit_service_version` to `ture` for such.
///
/// ## Reference
///
/// - [Blob, Queue, and File Services (Shared Key authorization)](https://docs.microsoft.com/en-us/rest/api/storageservices/authorize-with-shared-key)
fn string_to_sign(
    ctx: &mut SigningContext,
    ak: &str,
    now: DateTime,
    omit_service_version: bool,
) -> Result<String> {
    let mut s = String::with_capacity(128);

    writeln!(&mut s, "{}", ctx.method.as_str())?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&CONTENT_ENCODING)?)?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&CONTENT_LANGUAGE)?)?;
    writeln!(
        &mut s,
        "{}",
        ctx.header_get_or_default(&CONTENT_LENGTH)
            .map(|v| if v == "0" { "" } else { v })?
    )?;
    writeln!(
        &mut s,
        "{}",
        ctx.header_get_or_default(&CONTENT_MD5.parse()?)?
    )?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&CONTENT_TYPE)?)?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&DATE)?)?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&IF_MODIFIED_SINCE)?)?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&IF_MATCH)?)?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&IF_NONE_MATCH)?)?;
    writeln!(
        &mut s,
        "{}",
        ctx.header_get_or_default(&IF_UNMODIFIED_SINCE)?
    )?;
    writeln!(&mut s, "{}", ctx.header_get_or_default(&RANGE)?)?;
    writeln!(
        &mut s,
        "{}",
        canonicalize_header(ctx, now, omit_service_version)?
    )?;
    write!(&mut s, "{}", canonicalize_resource(ctx, ak))?;

    debug!("string to sign: {}", &s);

    Ok(s)
}

/// ## Reference
///
/// - [Constructing the canonicalized headers string](https://docs.microsoft.com/en-us/rest/api/storageservices/authorize-with-shared-key#constructing-the-canonicalized-headers-string)
fn canonicalize_header(
    ctx: &mut SigningContext,
    now: DateTime,
    omit_service_version: bool,
) -> Result<String> {
    ctx.headers
        .insert(X_MS_DATE, format_http_date(now).parse()?);
    if !omit_service_version {
        // Insert x_ms_version header.
        ctx.headers
            .insert(X_MS_VERSION, AZURE_VERSION.to_string().parse()?);
    }

    Ok(SigningContext::header_to_string(
        ctx.header_to_vec_with_prefix("x-ms-"),
        ":",
        "\n",
    ))
}

/// ## Reference
///
/// - [Constructing the canonicalized resource string](https://docs.microsoft.com/en-us/rest/api/storageservices/authorize-with-shared-key#constructing-the-canonicalized-resource-string)
fn canonicalize_resource(ctx: &mut SigningContext, ak: &str) -> String {
    if ctx.query.is_empty() {
        return format!("/{}{}", ak, ctx.path);
    }

    format!(
        "/{}{}\n{}",
        ak,
        ctx.path,
        SigningContext::query_to_string(ctx.query.clone(), ":", "\n")
    )
}

#[cfg(test)]
mod tests {
    use http::Request;

    use super::super::config::Config;
    use crate::azure::storage::loader::Loader;
    use crate::AzureStorageSigner;

    #[tokio::test]
    async fn test_sas_url() {
        let _ = env_logger::builder().is_test(true).try_init();

        let config = Config {
            sas_token: Some("sv=2021-01-01&ss=b&srt=c&sp=rwdlaciytfx&se=2022-01-01T11:00:14Z&st=2022-01-02T03:00:14Z&spr=https&sig=KEllk4N8f7rJfLjQCmikL2fRVt%2B%2Bl73UBkbgH%2FK3VGE%3D".to_string()),
            ..Default::default()
        };

        let loader = Loader::new(config);
        let cred = loader.load().await.unwrap().unwrap();

        let signer = AzureStorageSigner::new();

        // Construct request
        let mut req = Request::builder()
            .uri("https://test.blob.core.windows.net/testbucket/testblob")
            .body(())
            .unwrap();

        // Signing request with Signer
        assert!(signer.sign_query(&mut req, &cred).is_ok());
        assert_eq!(req.uri(), "https://test.blob.core.windows.net/testbucket/testblob?sv=2021-01-01&ss=b&srt=c&sp=rwdlaciytfx&se=2022-01-01T11:00:14Z&st=2022-01-02T03:00:14Z&spr=https&sig=KEllk4N8f7rJfLjQCmikL2fRVt%2B%2Bl73UBkbgH%2FK3VGE%3D")
    }
}
