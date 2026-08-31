use axum::{
    body::Body,
    extract::Request,
    http::{Method, StatusCode, header},
    middleware::Next,
    response::Response,
};

use crate::routes::maintenance::drain_remaining_millis;

fn should_reject(method: &Method, path: &str, remaining: u64) -> bool {
    let is_mutation = matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    is_mutation && !path.ends_with("/maintenance/drain") && remaining > 0
}

pub async fn reject_mutations_while_draining(request: Request, next: Next) -> Response {
    let remaining = drain_remaining_millis();
    if should_reject(request.method(), request.uri().path(), remaining) {
        let retry_after = remaining.div_ceil(1000).max(1).to_string();
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header(header::RETRY_AFTER, retry_after)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"success":false,"message":"cdesktop is draining for a verified local update; retry shortly"}"#,
            ))
            .unwrap_or_else(|_| Response::new(Body::empty()));
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_rejects_mutations_but_allows_reads_and_its_control_route() {
        assert!(should_reject(&Method::POST, "/sessions/1/follow-up", 1000));
        assert!(should_reject(&Method::DELETE, "/workspaces/1", 1000));
        assert!(!should_reject(&Method::GET, "/workspaces", 1000));
        assert!(!should_reject(&Method::POST, "/maintenance/drain", 1000));
        assert!(!should_reject(&Method::POST, "/sessions/1/follow-up", 0));
    }
}
