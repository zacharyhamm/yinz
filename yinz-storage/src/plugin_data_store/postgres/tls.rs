use native_tls::{Certificate, TlsConnector};
use percent_encoding::percent_decode_str;
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::{config::SslMode, Config};
use yinz_types::plugin_data::DataError;

use super::super::sql::{storage_err, StorageError};
use super::redact_url;

/// Rebuilt for every connection so a replaced CA file takes effect on reconnect.
pub(super) fn configuration(url: &str) -> Result<(Config, MakeTlsConnector), DataError> {
    let (url_without_ca, rootcert) = extract_rootcert(url)?;
    let config: Config = url_without_ca.parse().map_err(|e: tokio_postgres::Error| {
        e.into_data_error(&format!(
            "the plugin data store's Postgres URL {} does not parse",
            redact_url(url),
        ))
    })?;
    let mut builder = TlsConnector::builder();
    // Keep system roots and native-tls's certificate and hostname verification defaults.
    if let Some(path) = rootcert.filter(|_| config.get_ssl_mode() != SslMode::Disable) {
        let context = format!("failed to load Postgres sslrootcert {path:?}");
        let pem = std::fs::read_to_string(&path).map_err(|e| storage_err(&context, e))?;
        let mut count = 0;
        // native-tls 0.2.14 reads one certificate at a time on all supported platforms.
        // Check the envelope too: OpenSSL otherwise accepts junk around a valid certificate.
        for block in pem.split_inclusive("-----END CERTIFICATE-----") {
            let block = block.trim();
            if block.is_empty() {
                continue;
            }
            if !block.starts_with("-----BEGIN CERTIFICATE-----")
                || !block.ends_with("-----END CERTIFICATE-----")
            {
                return Err(storage_err(&context, "expected PEM certificates"));
            }
            let certificate =
                Certificate::from_pem(block.as_bytes()).map_err(|e| storage_err(&context, e))?;
            builder.add_root_certificate(certificate);
            count += 1;
        }
        if count == 0 {
            return Err(storage_err(&context, "no PEM certificates found"));
        }
    }
    postgres_native_tls::set_postgresql_alpn(&mut builder);
    let connector = builder
        .build()
        .map_err(|e| storage_err("failed to configure Postgres TLS", e))?;
    Ok((config, MakeTlsConnector::new(connector)))
}

/// Remove only our URL extension. Do not round-trip through a URL serializer: the driver
/// supports multi-host URLs, and its percent decoding preserves literal '+' characters.
fn extract_rootcert(url: &str) -> Result<(String, Option<String>), DataError> {
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        return Ok((url.to_owned(), None));
    }
    let Some((base, query)) = url.split_once('?') else {
        return Ok((url.to_owned(), None));
    };
    let mut retained = Vec::new();
    let mut rootcert = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode_str(key).decode_utf8().ok().as_deref() != Some("sslrootcert") {
            retained.push(pair);
            continue;
        }
        if rootcert.is_some() {
            return Err(storage_err(
                "invalid Postgres sslrootcert",
                "duplicate parameter",
            ));
        }
        let value = percent_decode_str(value)
            .decode_utf8()
            .map_err(|e| storage_err("invalid Postgres sslrootcert path", e))?;
        if value.is_empty() {
            return Err(storage_err("invalid Postgres sslrootcert", "empty path"));
        }
        rootcert = Some(value.into_owned());
    }
    let url = if retained.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", retained.join("&"))
    };
    Ok((url, rootcert))
}

#[cfg(test)]
#[path = "unit/tls.rs"]
mod tests;
