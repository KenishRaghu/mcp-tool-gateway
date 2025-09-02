//! Tracing setup. Spans are exported over OTLP/HTTP when
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set (e.g. `http://localhost:4318`), and the
//! incoming W3C `traceparent` header is honored so agent and gateway spans join
//! into one trace.

use axum::http::HeaderMap;
use opentelemetry::{global, propagation::Extractor, trace::TracerProvider as _};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::SdkTracerProvider, Resource};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init() -> Option<SdkTracerProvider> {
    global::set_text_map_propagator(TraceContextPropagator::new());
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let fmt = tracing_subscriber::fmt::layer().compact();

    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_err() {
        tracing_subscriber::registry().with(filter).with(fmt).init();
        return None;
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
        .expect("building OTLP exporter");
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(Resource::builder().with_service_name("mcp-gateway").build())
        .build();
    let tracer = provider.tracer("mcp-gateway");
    global::set_tracer_provider(provider.clone());

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt)
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();
    Some(provider)
}

/// Carrier holding just the W3C trace headers we care about.
struct TraceHeaders(Vec<(&'static str, String)>);

impl Extractor for TraceHeaders {
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }
    fn keys(&self) -> Vec<&str> {
        self.0.iter().map(|(k, _)| *k).collect()
    }
}

/// W3C Trace Context level 2 adds a "random" flag (0x02), so newer SDKs send
/// flags like `03`. Keep only the sampled bit so every propagator accepts it.
fn normalize_traceparent(v: &str) -> String {
    match v.rsplit_once('-') {
        Some((head, flags)) => match u8::from_str_radix(flags, 16) {
            Ok(f) => format!("{head}-{:02x}", f & 0x01),
            Err(_) => v.to_string(),
        },
        None => v.to_string(),
    }
}

/// Parent context propagated by the caller (agent) via `traceparent`.
pub fn parent_context(headers: &HeaderMap) -> opentelemetry::Context {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    let mut carrier = TraceHeaders(Vec::new());
    if let Some(tp) = get("traceparent") {
        carrier.0.push(("traceparent", normalize_traceparent(tp)));
    }
    if let Some(ts) = get("tracestate") {
        carrier.0.push(("tracestate", ts.to_string()));
    }
    global::get_text_map_propagator(|p| p.extract(&carrier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::TraceContextExt;

    #[test]
    fn accepts_level2_trace_flags() {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let mut h = HeaderMap::new();
        h.insert(
            "traceparent",
            "00-5d8d05910e6508de2d9e8094c4dfd240-7d1dd0e625d09976-03"
                .parse()
                .unwrap(),
        );
        let cx = parent_context(&h);
        assert_eq!(
            cx.span().span_context().trace_id().to_string(),
            "5d8d05910e6508de2d9e8094c4dfd240"
        );
        assert!(cx.span().span_context().is_sampled());
    }
}
