use std::fmt::Write;

pub fn normalize_method(method: &str) -> String {
    let method = method.trim().to_ascii_uppercase();
    if method.is_empty() {
        "ANY".to_owned()
    } else {
        method
    }
}

pub fn method_rank(method: &str) -> u8 {
    match normalize_method(method).as_str() {
        "GET" => 0,
        "POST" => 1,
        "PUT" => 2,
        "PATCH" => 3,
        "DELETE" => 4,
        "OPTIONS" => 5,
        "HEAD" => 6,
        "ANY" => 7,
        _ => 8,
    }
}

pub fn normalize_route_path(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_owned();
    }

    let mut raw = trimmed.replace('\\', "/");
    if !raw.starts_with('/') {
        raw.insert(0, '/');
    }

    let mut segments = Vec::new();
    for segment in raw.split('/') {
        if segment.is_empty() || is_route_group(segment) {
            continue;
        }
        segments.push(normalize_segment(segment));
    }

    if segments.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", segments.join("/"))
    }
}

/// Normalizes an `OpenAPI` path without applying framework-specific route syntax.
///
/// `OpenAPI` path keys are URL templates. Parenthesized segments, colon-prefixed
/// segments, repeated slashes, and a trailing slash are therefore significant
/// and must not be interpreted as Next.js or Express syntax.
pub fn normalize_openapi_route_path(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    }
}

fn is_route_group(segment: &str) -> bool {
    segment.starts_with('(') && segment.ends_with(')') && !segment.starts_with("(..")
}

fn normalize_segment(segment: &str) -> String {
    if let Some(name) = segment
        .strip_prefix("[[...")
        .and_then(|value| value.strip_suffix("]]"))
    {
        return format!("{{{name}?*}}");
    }
    if let Some(name) = segment
        .strip_prefix("[...")
        .and_then(|value| value.strip_suffix(']'))
    {
        return format!("{{{name}*}}");
    }
    if let Some(name) = segment
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        return format!("{{{name}}}");
    }
    if let Some(parameter) = segment.strip_prefix(':') {
        let (name, suffix) = parameter
            .strip_suffix('*')
            .map_or((parameter, ""), |value| (value, "*"));
        let (name, suffix) = name
            .strip_suffix('?')
            .map_or((name, suffix.to_owned()), |value| {
                (value, format!("?{suffix}"))
            });
        return format!("{{{name}{suffix}}}");
    }
    segment.to_owned()
}

pub fn district_for_path(path: &str) -> String {
    district_for_normalized_path(&normalize_route_path(path))
}

pub fn district_for_normalized_path(path: &str) -> String {
    path.split('/')
        .find(|segment| !segment.is_empty() && !segment.starts_with('{'))
        .map_or_else(|| "ROOT".to_owned(), str::to_ascii_uppercase)
}

pub fn canonical_endpoint_id(method: &str, path: &str) -> String {
    canonical_endpoint_id_for_normalized_path(method, &normalize_route_path(path))
}

pub fn canonical_endpoint_id_for_normalized_path(method: &str, path: &str) -> String {
    stable_id("ep", &format!("{} {path}", normalize_method(method)))
}

pub fn dependency_id(kind: crate::DependencyKind, name: &str) -> String {
    stable_id("dep", &format!("{kind:?}:{}", name.trim()))
}

pub fn schema_id(origin: &str) -> String {
    stable_id("schema", origin)
}

fn stable_id(prefix: &str, value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut output = String::with_capacity(prefix.len() + 18);
    output.push_str(prefix);
    output.push('-');
    write!(&mut output, "{hash:016x}").expect("writing to a string cannot fail");
    output
}

#[cfg(test)]
mod tests {
    use crate::{DependencyKind, dependency_id};

    use super::{
        canonical_endpoint_id_for_normalized_path, district_for_path, normalize_openapi_route_path,
        normalize_route_path,
    };

    #[test]
    fn normalizes_framework_route_syntax() {
        assert_eq!(
            normalize_route_path("app/(api)/users/[id]/"),
            "/app/users/{id}"
        );
        assert_eq!(normalize_route_path("/docs/[[...slug]]"), "/docs/{slug?*}");
        assert_eq!(normalize_route_path("users/:id"), "/users/{id}");
        assert_eq!(normalize_route_path("/files/:path*"), "/files/{path*}");
    }

    #[test]
    fn derives_a_stable_district() {
        assert_eq!(district_for_path("/users/{id}"), "USERS");
        assert_eq!(district_for_path("/{tenant}"), "ROOT");
    }

    #[test]
    fn preserves_literal_openapi_path_segments() {
        assert_eq!(
            normalize_openapi_route_path("/public/(admin)"),
            "/public/(admin)"
        );
        assert_eq!(normalize_openapi_route_path("/users/:id"), "/users/:id");
        assert_eq!(normalize_openapi_route_path("/admin/"), "/admin/");
        assert_eq!(normalize_openapi_route_path("/a//b"), "/a//b");
        assert_ne!(
            canonical_endpoint_id_for_normalized_path("GET", "/admin"),
            canonical_endpoint_id_for_normalized_path("GET", "/admin/")
        );
    }

    #[test]
    fn keeps_case_sensitive_dependency_names_distinct() {
        assert_ne!(
            dependency_id(DependencyKind::Middleware, "ApiKey"),
            dependency_id(DependencyKind::Middleware, "apikey")
        );
    }
}
