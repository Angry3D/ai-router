use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use futures_util::StreamExt as _;
use reqwest::{Client, StatusCode, header, redirect::Policy};
use tokio::time::{Instant, timeout_at};
use url::{Host, Url};

use super::asset::{ImageAssetErrorKind, MAX_COMPRESSED_PNG_BYTES};
use crate::proxy::upstream::{DecodeError, decode_supported_exact, response_encodings};

pub(super) const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_DNS_ADDRESSES: usize = 16;
const MAX_REDIRECTS: usize = 3;

#[cfg(test)]
pub(in crate::proxy) mod test_support;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
struct DownloadLimits {
    total_timeout: Duration,
    dns_timeout: Duration,
    connect_timeout: Duration,
    body_bytes: usize,
}

impl Default for DownloadLimits {
    fn default() -> Self {
        Self {
            total_timeout: Duration::from_mins(10),
            dns_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(30),
            body_bytes: MAX_COMPRESSED_PNG_BYTES,
        }
    }
}

// No client, route, credential, cookie store, or URL survives a download call.
#[derive(Clone, Default)]
pub(in crate::proxy) struct ImageAssetDownloader {
    limits: DownloadLimits,
    #[cfg(test)]
    network: Option<std::sync::Arc<test_support::TestNetwork>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ImageDownloadError {
    pub(super) kind: ImageAssetErrorKind,
    pub(super) upstream_status: Option<StatusCode>,
}

impl ImageDownloadError {
    const fn new(kind: ImageAssetErrorKind, upstream_status: Option<StatusCode>) -> Self {
        Self {
            kind,
            upstream_status,
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

// Keeping the parsed URL itself ensures admission, DNS, Host, SNI, and the
// eventual request all use the same normalized target, including IDNA names.
struct AdmittedUrl(Url);

impl AdmittedUrl {
    fn parse(raw: &str) -> Result<Self, ()> {
        validate_url_text(raw)?;
        if !raw
            .split_once("://")
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
        {
            return Err(());
        }
        Self::from_url(Url::parse(raw).map_err(|_| ())?)
    }

    fn from_url(url: Url) -> Result<Self, ()> {
        if url.as_str().len() > MAX_URL_BYTES
            || url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.host().is_none()
        {
            return Err(());
        }
        Ok(Self(url))
    }

    fn redirect(&self, location: &str) -> Result<Self, ()> {
        validate_url_text(location)?;
        Self::from_url(self.0.join(location).map_err(|_| ())?)
    }
}

fn validate_url_text(raw: &str) -> Result<(), ()> {
    if raw.is_empty()
        || raw.len() > MAX_URL_BYTES
        || raw.trim() != raw
        || raw.chars().any(char::is_control)
        || raw.contains('\\')
    {
        return Err(());
    }
    // Url normalizes empty userinfo away; reject its delimiter before parsing.
    let authority = if let Some(authority) = raw.strip_prefix("//") {
        Some(authority)
    } else if let Some((_, remainder)) = raw.split_once(':').filter(|(scheme, _)| {
        !scheme.is_empty()
            && scheme.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_alphabetic()
                    || (index > 0 && (byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')))
            })
    }) {
        Some(remainder.strip_prefix("//").ok_or(())?)
    } else {
        None
    };
    if authority.is_some_and(|authority| {
        authority
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.is_empty() || authority.contains('@'))
    }) {
        return Err(());
    }
    Ok(())
}

fn normalized_address(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address @ IpAddr::V4(_) => address,
    }
}

fn public_ipv4(address: Ipv4Addr) -> bool {
    const EXCLUDED: [(u32, u32); 15] = [
        (0x0000_0000, 8),
        (0x0a00_0000, 8),
        (0x6440_0000, 10),
        (0x7f00_0000, 8),
        (0xa9fe_0000, 16),
        (0xac10_0000, 12),
        (0xc000_0000, 24),
        (0xc000_0200, 24),
        (0xc058_6300, 24),
        (0xc0a8_0000, 16),
        (0xc612_0000, 15),
        (0xc633_6400, 24),
        (0xcb00_7100, 24),
        (0xe000_0000, 4),
        (0xf000_0000, 4),
    ];
    let bits = address.to_bits();
    !EXCLUDED
        .iter()
        .any(|&(network, prefix)| bits & (u32::MAX << (32 - prefix)) == network)
}

fn public_ipv6(address: Ipv6Addr) -> bool {
    let [first, second, ..] = address.segments();
    first & 0xe000 == 0x2000
        && !(first == 0x2001 && second & 0xfe00 == 0)
        && !(first == 0x2001 && second == 0x0db8)
        && first != 0x2002
        && !(first == 0x3fff && second & 0xf000 == 0)
}

fn admit_addresses(addresses: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, ()> {
    if addresses.is_empty() || addresses.len() > MAX_DNS_ADDRESSES {
        return Err(());
    }
    addresses
        .into_iter()
        .map(|address| {
            let ip = normalized_address(address.ip());
            let public = match ip {
                IpAddr::V4(address) => public_ipv4(address),
                IpAddr::V6(address) => public_ipv6(address),
            };
            if public && address.port() == 443 {
                Ok(SocketAddr::new(ip, 443))
            } else {
                Err(())
            }
        })
        .collect()
}

fn redirect_target(
    target: &AdmittedUrl,
    response: &reqwest::Response,
    followed: usize,
    visited: &[Url],
) -> Result<AdmittedUrl, ImageDownloadError> {
    let error = |kind| ImageDownloadError::new(kind, Some(response.status()));
    if followed == MAX_REDIRECTS {
        return Err(error(ImageAssetErrorKind::DownloadFailed));
    }
    let location = response
        .headers()
        .get(header::LOCATION)
        .ok_or_else(|| error(ImageAssetErrorKind::DownloadFailed))?;
    let next = location
        .to_str()
        .map_err(|_| ())
        .and_then(|location| target.redirect(location))
        .map_err(|()| error(ImageAssetErrorKind::InvalidUrl))?;
    if visited.contains(&next.0) {
        return Err(error(ImageAssetErrorKind::DownloadFailed));
    }
    Ok(next)
}

impl ImageAssetDownloader {
    pub(super) async fn download(
        &self,
        raw_url: String,
        generation_status: StatusCode,
    ) -> Result<DownloadedImage, ImageDownloadError> {
        let deadline = Instant::now() + self.limits.total_timeout;
        let mut target = AdmittedUrl::parse(&raw_url).map_err(|()| {
            ImageDownloadError::new(ImageAssetErrorKind::InvalidUrl, Some(generation_status))
        })?;
        drop(raw_url);
        let mut source_status = generation_status;
        let mut visited = vec![target.0.clone()];

        for redirects in 0..=MAX_REDIRECTS {
            let addresses = timeout_at(
                deadline.min(Instant::now() + self.limits.dns_timeout),
                self.resolve(&target),
            )
            .await
            .map_err(|_| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))?
            .map_err(|()| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))?;
            let addresses = admit_addresses(addresses).map_err(|()| {
                ImageDownloadError::new(ImageAssetErrorKind::InvalidUrl, Some(source_status))
            })?;

            // In tests only, map these already-approved logical addresses onto
            // a local TLS fixture. Production connects to exactly this DNS set.
            #[cfg(test)]
            let addresses = self.network.as_ref().map_or(addresses.clone(), |network| {
                network.connect_addresses(&target, &addresses)
            });

            let client = self.client(&target, &addresses, deadline)?;
            let response = timeout_at(
                deadline,
                client
                    .get(target.0.clone())
                    .header(header::ACCEPT_ENCODING, "identity")
                    .send(),
            )
            .await
            .map_err(|_| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))?
            .map_err(|_| ImageDownloadError::new(ImageAssetErrorKind::DownloadFailed, None))?;
            let status = response.status();
            if response.remote_addr().is_some_and(|peer| {
                let peer = SocketAddr::new(normalized_address(peer.ip()), peer.port());
                !addresses.contains(&peer)
            }) {
                return Err(ImageDownloadError::new(
                    ImageAssetErrorKind::InvalidUrl,
                    Some(status),
                ));
            }

            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                let next = redirect_target(&target, &response, redirects, &visited)?;
                visited.push(next.0.clone());
                target = next;
                source_status = status;
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
            Some(source_status),
        ))
    }

    async fn resolve(&self, target: &AdmittedUrl) -> Result<Vec<SocketAddr>, ()> {
        match target.0.host().ok_or(())? {
            Host::Ipv4(address) => Ok(vec![SocketAddr::new(IpAddr::V4(address), 443)]),
            Host::Ipv6(address) => Ok(vec![SocketAddr::new(IpAddr::V6(address), 443)]),
            Host::Domain(domain) => {
                #[cfg(test)]
                if let Some(network) = &self.network {
                    return network.resolve(domain).await;
                }
                tokio::net::lookup_host((domain, 443))
                    .await
                    .map(|addresses| addresses.take(MAX_DNS_ADDRESSES + 1).collect())
                    .map_err(|_| ())
            }
        }
    }

    fn client(
        &self,
        target: &AdmittedUrl,
        addresses: &[SocketAddr],
        deadline: Instant,
    ) -> Result<Client, ImageDownloadError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let mut builder = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(Policy::none())
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
        if let Some(network) = &self.network {
            builder = network.configure_client(builder);
        }
        if let Some(Host::Domain(domain)) = target.0.host() {
            builder = builder.resolve_to_addrs(domain, addresses);
        }
        builder
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
