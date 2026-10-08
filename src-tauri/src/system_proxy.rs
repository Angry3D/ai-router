use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    number::CFNumber,
    propertylist::{CFPropertyListSubClass, create_data, kCFPropertyListBinaryFormat_v1_0},
    runloop::{CFRunLoop, CFRunLoopRunResult},
    string::CFString,
};
use router_core::proxy::{OutboundProxyTransport, SystemProxyError, SystemProxySettings};
use system_configuration::dynamic_store::{
    SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext,
};
use url::Url;

/// Owns one native notification source and its worker for the desktop lifetime.
/// No request performs native reads; only initial subscription and notifications do.
pub(crate) struct SystemProxyMonitor {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SystemProxyMonitor {
    pub(crate) fn start(transport: OutboundProxyTransport) -> Option<Self> {
        // Until the first native read succeeds, absence of a policy is not direct access.
        transport.set_system_settings(Err(SystemProxyError::ReadFailed));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_transport = transport;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("system-proxy-settings".to_owned())
            .spawn(move || {
                if watch(&worker_transport, &worker_stop, &ready_tx).is_err() {
                    worker_transport.set_system_settings(Err(SystemProxyError::ReadFailed));
                }
            })
            .ok()?;
        // Bound startup even if the native service is not responding. A late successful
        // read can recover through the same worker; settings and Custom remain usable.
        let _ = ready_rx.recv_timeout(Duration::from_secs(2));
        Some(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for SystemProxyMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn watch(
    transport: &OutboundProxyTransport,
    stop: &AtomicBool,
    ready: &mpsc::SyncSender<()>,
) -> Result<(), SystemProxyError> {
    let store = SCDynamicStoreBuilder::new("AI Router system proxy")
        .callback_context(SCDynamicStoreCallBackContext {
            callout: |store, _changed_keys, transport: &mut OutboundProxyTransport| {
                transport.set_system_settings(read_settings(&store));
            },
            info: transport.clone(),
        })
        .build()
        .ok_or(SystemProxyError::ReadFailed)?;
    let keys = CFArray::from_CFTypes(&[
        CFString::new("State:/Network/Global/Proxies"),
        CFString::new("State:/Network/Global/IPv4"),
        CFString::new("State:/Network/Global/IPv6"),
    ]);
    let patterns = CFArray::from_CFTypes(&[
        CFString::new("State:/Network/Service/.*/Proxies"),
        CFString::new("Setup:/Network/.*"),
    ]);
    if !store.set_notification_keys(&keys, &patterns) {
        return Err(SystemProxyError::ReadFailed);
    }
    let source = store
        .create_run_loop_source()
        .ok_or(SystemProxyError::ReadFailed)?;
    let run_loop = CFRunLoop::get_current();
    // A private mode avoids unsafe access to the framework's global mode pointer.
    let mode = CFString::new("AI Router system proxy notifications");
    run_loop.add_source(&source, mode.as_concrete_TypeRef());
    transport.set_system_settings(read_settings(&store));
    let _ = ready.send(());
    while !stop.load(Ordering::Acquire) {
        // This pumps notifications, not settings polling, and bounds normal teardown.
        if CFRunLoop::run_in_mode(mode.as_concrete_TypeRef(), Duration::from_millis(250), true)
            == CFRunLoopRunResult::Finished
        {
            run_loop.remove_source(&source, mode.as_concrete_TypeRef());
            return Err(SystemProxyError::ReadFailed);
        }
    }
    run_loop.remove_source(&source, mode.as_concrete_TypeRef());
    Ok(())
}

fn read_settings(store: &SCDynamicStore) -> Result<SystemProxySettings, SystemProxyError> {
    let dictionary = store.get_proxies().ok_or(SystemProxyError::ReadFailed)?;
    parse_settings(&dictionary)
}

fn parse_settings(
    dictionary: &CFDictionary<CFString, CFType>,
) -> Result<SystemProxySettings, SystemProxyError> {
    // Read only these static values, never PAC URLs/scripts, credentials, or Keychain.
    let pac_enabled = enabled(dictionary, "ProxyAutoConfigEnable")?;
    let discovery_enabled = enabled(dictionary, "ProxyAutoDiscoveryEnable")?;
    Ok(SystemProxySettings {
        http: endpoint(dictionary, "HTTPEnable", "HTTPProxy", "HTTPPort", "http")?,
        // HTTPS describes the destination protocol; macOS's endpoint is an HTTP CONNECT proxy.
        https: endpoint(dictionary, "HTTPSEnable", "HTTPSProxy", "HTTPSPort", "http")?,
        socks: endpoint(
            dictionary,
            "SOCKSEnable",
            "SOCKSProxy",
            "SOCKSPort",
            "socks5h",
        )?,
        bypass: exceptions(dictionary)?,
        exclude_simple_hostnames: enabled(dictionary, "ExcludeSimpleHostnames")?,
        automatic_enabled: pac_enabled || discovery_enabled,
    })
}

fn enabled(
    dictionary: &CFDictionary<CFString, CFType>,
    key: &str,
) -> Result<bool, SystemProxyError> {
    let Some(value) = dictionary.find(CFString::new(key)) else {
        return Ok(false);
    };
    if let Some(value) = value.downcast::<CFBoolean>() {
        return Ok(value.into());
    }
    match value
        .downcast::<CFNumber>()
        .and_then(|value| value.to_i64())
    {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(SystemProxyError::InvalidSettings),
    }
}

fn endpoint(
    dictionary: &CFDictionary<CFString, CFType>,
    enable_key: &str,
    host_key: &str,
    port_key: &str,
    scheme: &str,
) -> Result<Option<Url>, SystemProxyError> {
    if !enabled(dictionary, enable_key)? {
        return Ok(None);
    }
    let host = dictionary
        .find(CFString::new(host_key))
        .and_then(|value| value.downcast::<CFString>())
        .ok_or(SystemProxyError::InvalidSettings)?
        .to_string();
    let port = dictionary
        .find(CFString::new(port_key))
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|value| value.to_i64())
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or(SystemProxyError::InvalidSettings)?;
    if host.is_empty()
        || host
            .chars()
            .any(|value| value.is_whitespace() || value.is_control())
        || host.contains(['/', '@', '?', '#'])
    {
        return Err(SystemProxyError::InvalidSettings);
    }
    let host = if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host
    };
    let url = Url::parse(&format!("{scheme}://{host}:{port}"))
        .map_err(|_| SystemProxyError::InvalidSettings)?;
    if url.host_str().is_none() || url.port_or_known_default() != Some(port) {
        return Err(SystemProxyError::InvalidSettings);
    }
    Ok(Some(url))
}

fn exceptions(
    dictionary: &CFDictionary<CFString, CFType>,
) -> Result<Vec<String>, SystemProxyError> {
    let Some(value) = dictionary.find(CFString::new("ExceptionsList")) else {
        return Ok(Vec::new());
    };
    let array = value
        .downcast::<CFArray>()
        .ok_or(SystemProxyError::InvalidSettings)?;
    // core-foundation's safe downcast only exposes an untyped array. Encode this
    // one value in memory to traverse it safely; never serialize the whole dictionary.
    let data = create_data(
        array.to_CFPropertyList().as_concrete_TypeRef(),
        kCFPropertyListBinaryFormat_v1_0,
    )
    .map_err(|_| SystemProxyError::InvalidSettings)?;
    let value = plist::Value::from_reader(Cursor::new(data.bytes()))
        .map_err(|_| SystemProxyError::InvalidSettings)?;
    let plist::Value::Array(values) = value else {
        return Err(SystemProxyError::InvalidSettings);
    };
    values
        .into_iter()
        .map(|value| match value {
            plist::Value::String(value) => Ok(value),
            _ => Err(SystemProxyError::InvalidSettings),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(values: &[(&str, CFType)]) -> CFDictionary<CFString, CFType> {
        CFDictionary::from_CFType_pairs(
            &values
                .iter()
                .map(|(key, value)| (CFString::new(key), value.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn number(value: i32) -> CFType {
        CFNumber::from(value).into_CFType()
    }

    fn string(value: &str) -> CFType {
        CFString::new(value).into_CFType()
    }

    #[test]
    fn static_dictionary_preserves_manual_and_automatic_settings() {
        let settings = parse_settings(&dictionary(&[
            ("HTTPEnable", number(1)),
            ("HTTPProxy", string("proxy.invalid")),
            ("HTTPPort", number(8080)),
            ("HTTPSEnable", number(1)),
            ("HTTPSProxy", string("2001:db8::5")),
            ("HTTPSPort", number(8443)),
            ("SOCKSEnable", number(1)),
            ("SOCKSProxy", string("socks.invalid")),
            ("SOCKSPort", number(1080)),
            ("ProxyAutoConfigEnable", number(1)),
            (
                "ProxyAutoConfigURLString",
                string("http://must-not-fetch.invalid/private.pac"),
            ),
            (
                "ExcludeSimpleHostnames",
                CFBoolean::from(true).into_CFType(),
            ),
            (
                "ExceptionsList",
                CFArray::from_CFTypes(&[
                    CFString::new("*.example.invalid"),
                    CFString::new("192.0.2.0/24"),
                ])
                .into_CFType(),
            ),
        ]))
        .expect("synthetic settings");
        assert_eq!(
            settings.http.unwrap().as_str(),
            "http://proxy.invalid:8080/"
        );
        assert_eq!(
            settings.https.unwrap().as_str(),
            "http://[2001:db8::5]:8443/"
        );
        assert_eq!(
            settings.socks.unwrap().as_str(),
            "socks5h://socks.invalid:1080"
        );
        assert_eq!(settings.bypass, ["*.example.invalid", "192.0.2.0/24"]);
        assert!(settings.exclude_simple_hostnames);
        assert!(settings.automatic_enabled);
    }

    #[test]
    fn disabled_values_are_ignored_but_active_malformed_values_fail_closed() {
        assert_eq!(
            parse_settings(&dictionary(&[])),
            Ok(SystemProxySettings::default())
        );
        assert_eq!(
            parse_settings(&dictionary(&[
                ("HTTPEnable", number(0)),
                ("HTTPProxy", number(9)),
                ("HTTPPort", number(-1)),
                ("ProxyAutoConfigEnable", number(0)),
                ("ProxyAutoConfigURLString", string("not a URL")),
            ])),
            Ok(SystemProxySettings::default())
        );
        for values in [
            vec![("HTTPEnable", number(1))],
            vec![("ProxyAutoDiscoveryEnable", string("enabled"))],
            vec![("ExceptionsList", string("*.invalid"))],
            vec![(
                "ExceptionsList",
                CFArray::from_CFTypes(&[number(1)]).into_CFType(),
            )],
        ] {
            assert_eq!(
                parse_settings(&dictionary(&values)),
                Err(SystemProxyError::InvalidSettings)
            );
        }
        for (host, port) in [
            ("proxy.invalid", 0),
            ("proxy.invalid", 65536),
            ("user@proxy.invalid", 80),
            ("proxy.invalid/path", 80),
        ] {
            assert_eq!(
                parse_settings(&dictionary(&[
                    ("HTTPEnable", number(1)),
                    ("HTTPProxy", string(host)),
                    ("HTTPPort", number(port)),
                ])),
                Err(SystemProxyError::InvalidSettings)
            );
        }
        assert!(
            parse_settings(&dictionary(&[("ProxyAutoDiscoveryEnable", number(1))]))
                .expect("WPAD flag")
                .automatic_enabled
        );
    }
}
