use std::time::Duration;

use futures_util::StreamExt as _;
use reqwest::{Client, StatusCode, header, redirect::Policy};
use tokio::time::{Instant, timeout_at};
use url::Url;

use super::asset::{ImageAssetErrorKind, MAX_COMPRESSED_PNG_BYTES};
use crate::proxy::{
    OutboundProxyPolicy, OutboundProxyTransport, SystemProxyError,
    upstream::{DecodeError, decode_supported_exact, response_encodings},
};

const MAX_REDIRECTS: usize = 3;

#[cfg(test)]
pub(in crate::proxy) mod test_support;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
struct DownloadLimits {
    total_timeout: Duration,
    connect_timeout: Duration,
    body_bytes: usize,
}

impl Default for DownloadLimits {
    fn default() -> Self {
        Self {
            total_timeout: Duration::from_mins(10),
            connect_timeout: Duration::from_secs(30),
            body_bytes: MAX_COMPRESSED_PNG_BYTES,
        }
    }
}

// Credentials and clients never survive a download call; only policy is shared.
#[derive(Clone)]
pub(in crate::proxy) struct ImageAssetDownloader {
    limits: DownloadLimits,
    outbound_proxy: OutboundProxyTransport,
    #[cfg(test)]
    network: Option<std::sync::Arc<test_support::TestNetwork>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ImageDownloadError {
    pub(super) kind: ImageAssetErrorKind,
    pub(super) upstream_status: Option<StatusCode>,
    pub(super) system_proxy: Option<SystemProxyError>,
}

impl ImageDownloadError {
    const fn new(kind: ImageAssetErrorKind, upstream_status: Option<StatusCode>) -> Self {
        Self {
            kind,
            upstream_status,
            system_proxy: None,
        }
    }
}

// Wire bytes and encodings are deliberately private and never formatted.
// The caller moves this value, together with its memory permit, into blocking
// decoding/publication. Dropping an async download cannot detach body decoding.
pub(super) struct DownloadedImage {
    pub(super) upstream_status: StatusCode,
    wire: Vec<u8>,
    encodings: Vec<String>,
    limit: usize,
}

impl DownloadedImage {
    pub(super) fn decode(self) -> Result<Vec<u8>, ImageDownloadError> {
        decode_supported_exact(self.wire, &self.encodings, self.limit).map_err(|error| {
            ImageDownloadError::new(
                match error {
                    DecodeError::TooLarge => ImageAssetErrorKind::TooLarge,
                    DecodeError::Invalid | DecodeError::Unsupported => {
                        ImageAssetErrorKind::DownloadFailed
                    }
                },
                Some(self.upstream_status),
            )
        })
    }
}

fn redirect_target(
    target: &Url,
    response: &reqwest::Response,
    followed: usize,
    visited: &[Url],
) -> Result<Url, ImageDownloadError> {
    let error = |kind| ImageDownloadError::new(kind, Some(response.status()));
    if followed == MAX_REDIRECTS {
        return Err(error(ImageAssetErrorKind::DownloadFailed));
    }
    let location = response
        .headers()
        .get(header::LOCATION)
        .ok_or_else(|| error(ImageAssetErrorKind::DownloadFailed))?;
    let location = location
        .to_str()
        .map_err(|_| error(ImageAssetErrorKind::DownloadFailed))?;
    let next = target
        .join(location)
        .map_err(|_| error(ImageAssetErrorKind::DownloadFailed))?;
    if visited.contains(&next) {
        return Err(error(ImageAssetErrorKind::DownloadFailed));
    }
    Ok(next)
}

impl ImageAssetDownloader {
    pub(in crate::proxy) fn new(outbound_proxy: OutboundProxyTransport) -> Self {
        Self {
            limits: DownloadLimits::default(),
            outbound_proxy,
            #[cfg(test)]
            network: None,
        }
    }

    #[cfg(test)]
    pub(in crate::proxy) fn with_outbound_proxy(
        mut self,
        outbound_proxy: OutboundProxyTransport,
    ) -> Self {
        self.outbound_proxy = outbound_proxy;
        self
    }

    pub(super) async fn download(
        &self,
        raw_url: String,
        generation_status: StatusCode,
    ) -> Result<DownloadedImage, ImageDownloadError> {
        let deadline = Instant::now() + self.limits.total_timeout;
        let mut target = Url::parse(&raw_url)
            .map_err(|_| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))?;
        drop(raw_url);
        let policy = self.outbound_proxy.policy();
        let mut visited = vec![target.clone()];

        for redirects in 0..=MAX_REDIRECTS {
            policy
                .validate_target(&target)
                .map_err(|error| ImageDownloadError {
                    kind: ImageAssetErrorKind::DownloadFailed,
                    upstream_status: None,
                    system_proxy: Some(error),
                })?;
            let client = self.client(deadline, &policy)?;
            let response = timeout_at(
                deadline,
                client
                    .get(target.clone())
                    .header(header::ACCEPT_ENCODING, "identity")
                    .send(),
            )
            .await
            .map_err(|_| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))?
            .map_err(|_| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))?;
            let status = response.status();

            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                let next = redirect_target(&target, &response, redirects, &visited)?;
                visited.push(next.clone());
                target = next;
                continue;
            }
            if !status.is_success() {
                return Err(ImageDownloadError::new(
                    ImageAssetErrorKind::DownloadFailed,
                    Some(status),
                ));
            }
            return timeout_at(deadline, self.read_body(response))
                .await
                .map_err(|_| {
                    ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, Some(status))
                })?;
        }
        Err(ImageDownloadError::new(
            ImageAssetErrorKind::DownloadFailed,
            Some(generation_status),
        ))
    }

    fn client(
        &self,
        deadline: Instant,
        policy: &OutboundProxyPolicy,
    ) -> Result<Client, ImageDownloadError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let builder = Client::builder()
            .retry(reqwest::retry::never())
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .pool_max_idle_per_host(0)
            .timeout(remaining)
            .connect_timeout(self.limits.connect_timeout.min(remaining));
        #[cfg(test)]
        let builder = match &self.network {
            Some(network) => network.configure_client(builder),
            None => builder,
        };
        policy
            .configure_client(builder, Policy::none())
            // This downloader validates and follows each hop explicitly.
            .redirect(Policy::none())
            .build()
            .map_err(|_| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))
    }

    async fn read_body(
        &self,
        response: reqwest::Response,
    ) -> Result<DownloadedImage, ImageDownloadError> {
        let status = response.status();
        let error = |kind| ImageDownloadError::new(kind, Some(status));
        let limit = self.limits.body_bytes;
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(error(ImageAssetErrorKind::TooLarge));
        }
        let encodings = response_encodings(response.headers());
        let mut stream = response.bytes_stream();
        let mut wire = Vec::new();
        wire.try_reserve_exact(limit)
            .map_err(|_| error(ImageAssetErrorKind::TooLarge))?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| error(ImageAssetErrorKind::DownloadFailed))?;
            if wire.len().saturating_add(chunk.len()) > limit {
                return Err(error(ImageAssetErrorKind::TooLarge));
            }
            wire.extend_from_slice(&chunk);
        }
        Ok(DownloadedImage {
            upstream_status: status,
            wire,
            encodings,
            limit,
        })
    }
}
