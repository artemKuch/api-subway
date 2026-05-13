use std::path::Path;

use api_subway_core::DependencyKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalBoundary {
    pub name: String,
    pub kind: DependencyKind,
    pub role: &'static str,
}

pub(crate) fn classify_local_module(
    relative: &str,
    called_symbol: Option<&str>,
) -> Option<LocalBoundary> {
    let normalized = relative.replace('\\', "/");
    let path = Path::new(&normalized);
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    let stem = path.file_stem()?.to_str()?;
    let mut tokens = components
        .iter()
        .flat_map(|component| identifier_tokens(component))
        .collect::<Vec<_>>();
    tokens.extend(identifier_tokens(stem));

    let (kind, role, markers) =
        if contains_any(&tokens, &["middleware", "middlewares", "guard", "guards"]) {
            (
                DependencyKind::Middleware,
                "middleware",
                &["middleware", "middlewares", "guard", "guards"][..],
            )
        } else if contains_any(
            &tokens,
            &[
                "repository",
                "repositories",
                "repo",
                "repos",
                "dao",
                "persistence",
                "database",
                "databases",
            ],
        ) {
            (
                DependencyKind::Datastore,
                "repository",
                &[
                    "repository",
                    "repositories",
                    "repo",
                    "repos",
                    "dao",
                    "persistence",
                    "database",
                    "databases",
                ][..],
            )
        } else if contains_any(
            &tokens,
            &[
                "client",
                "clients",
                "gateway",
                "gateways",
                "integration",
                "integrations",
                "provider",
                "providers",
            ],
        ) {
            (
                DependencyKind::External,
                "integration client",
                &[
                    "client",
                    "clients",
                    "gateway",
                    "gateways",
                    "integration",
                    "integrations",
                    "provider",
                    "providers",
                ][..],
            )
        } else if contains_any(
            &tokens,
            &[
                "service",
                "services",
                "usecase",
                "usecases",
                "interactor",
                "interactors",
            ],
        ) {
            (
                DependencyKind::Service,
                "service",
                &[
                    "service",
                    "services",
                    "usecase",
                    "usecases",
                    "interactor",
                    "interactors",
                ][..],
            )
        } else {
            return None;
        };

    let entity = boundary_entity(&components, stem, called_symbol, markers);
    Some(LocalBoundary {
        name: title_case(&entity),
        kind,
        role,
    })
}

fn boundary_entity(
    components: &[&str],
    stem: &str,
    called_symbol: Option<&str>,
    markers: &[&str],
) -> String {
    let generic = ["index", "mod", "main", "default"];
    let stem_tokens = identifier_tokens(stem)
        .into_iter()
        .filter(|token| !markers.contains(&token.as_str()) && !generic.contains(&token.as_str()))
        .collect::<Vec<_>>();
    if !stem_tokens.is_empty() {
        return stem_tokens.join(" ");
    }

    if let Some(symbol) = called_symbol {
        let symbol_tokens = identifier_tokens(symbol)
            .into_iter()
            .filter(|token| {
                !markers.contains(&token.as_str())
                    && !generic.contains(&token.as_str())
                    && !is_action_verb(token)
            })
            .collect::<Vec<_>>();
        if !symbol_tokens.is_empty() {
            return symbol_tokens.join(" ");
        }
    }

    components
        .iter()
        .rev()
        .flat_map(|component| identifier_tokens(component))
        .find(|token| {
            !markers.contains(&token.as_str())
                && !generic.contains(&token.as_str())
                && !matches!(token.as_str(), "src" | "app" | "lib")
        })
        .unwrap_or_else(|| "Local dependency".to_owned())
}

fn identifier_tokens(value: &str) -> Vec<String> {
    let mut normalized = String::with_capacity(value.len() + 8);
    let mut previous_was_lowercase = false;
    for character in value.chars() {
        if character.is_ascii_uppercase() && previous_was_lowercase {
            normalized.push(' ');
        }
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
            previous_was_lowercase = character.is_ascii_lowercase() || character.is_ascii_digit();
        } else {
            normalized.push(' ');
            previous_was_lowercase = false;
        }
    }
    normalized.split_whitespace().map(str::to_owned).collect()
}

fn contains_any(tokens: &[String], candidates: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| candidates.contains(&token.as_str()))
}

fn is_action_verb(value: &str) -> bool {
    matches!(
        value,
        "create"
            | "delete"
            | "fetch"
            | "find"
            | "get"
            | "list"
            | "load"
            | "read"
            | "remove"
            | "save"
            | "send"
            | "update"
            | "write"
    )
}

fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                format!("{}{}", first.to_ascii_uppercase(), characters.as_str())
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use api_subway_core::DependencyKind;

    use super::classify_local_module;

    #[test]
    fn classifies_common_module_boundaries() {
        let service = classify_local_module("src/services/users.ts", Some("loadUser"))
            .expect("service boundary");
        assert_eq!(service.name, "Users");
        assert_eq!(service.kind, DependencyKind::Service);

        let repository =
            classify_local_module("app/repositories/order_repository.py", Some("save_order"))
                .expect("repository boundary");
        assert_eq!(repository.name, "Order");
        assert_eq!(repository.kind, DependencyKind::Datastore);

        let client = classify_local_module("src/clients/index.ts", Some("sendNotification"))
            .expect("client boundary");
        assert_eq!(client.name, "Notification");
        assert_eq!(client.kind, DependencyKind::External);
    }

    #[test]
    fn ignores_plain_helper_modules() {
        assert!(classify_local_module("src/lib/format.ts", Some("formatUser")).is_none());
    }
}
