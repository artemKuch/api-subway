use std::collections::BTreeMap;

use api_subway_core::{ApiSubwayConfig, DependencyRule};
use globset::{Glob, GlobSet, GlobSetBuilder};

pub(crate) const MAX_CUSTOM_RULE_MATCHES: usize = 100_000;

#[derive(Debug, Clone)]
pub(crate) struct CustomRuleMatch {
    pub rule_index: usize,
    pub line: u32,
    pub detail: String,
    pub package_match: bool,
}

#[derive(Debug)]
pub(crate) struct CompiledDependencyRules {
    rules: Vec<DependencyRule>,
    path_set: Option<GlobSet>,
    path_rule_indices: Vec<usize>,
    package_rule_indices: BTreeMap<String, Vec<usize>>,
}

impl CompiledDependencyRules {
    pub fn new(config: &ApiSubwayConfig) -> Result<Self, String> {
        let mut path_builder = GlobSetBuilder::new();
        let mut path_rule_indices = Vec::new();
        let mut package_rule_indices = BTreeMap::<String, Vec<usize>>::new();
        for (rule_index, rule) in config.dependencies.iter().enumerate() {
            for pattern in &rule.path_globs {
                let glob = Glob::new(pattern).map_err(|error| error.to_string())?;
                path_builder.add(glob);
                path_rule_indices.push(rule_index);
            }
            for package in &rule.packages {
                package_rule_indices
                    .entry(package.clone())
                    .or_default()
                    .push(rule_index);
            }
        }
        for indices in package_rule_indices.values_mut() {
            indices.sort_unstable();
            indices.dedup();
        }
        let path_set = if path_rule_indices.is_empty() {
            None
        } else {
            Some(path_builder.build().map_err(|error| error.to_string())?)
        };
        Ok(Self {
            rules: config.dependencies.clone(),
            path_set,
            path_rule_indices,
            package_rule_indices,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rule(&self, index: usize) -> Option<&DependencyRule> {
        self.rules.get(index)
    }

    pub fn match_file<'a>(
        &self,
        relative_path: &str,
        imports: impl IntoIterator<Item = (&'a str, u32)>,
        package_separator: char,
    ) -> Vec<CustomRuleMatch> {
        let mut matches = BTreeMap::<usize, CustomRuleMatch>::new();
        if let Some(path_set) = &self.path_set {
            for pattern_index in path_set.matches(relative_path) {
                let rule_index = self.path_rule_indices[pattern_index];
                matches.entry(rule_index).or_insert(CustomRuleMatch {
                    rule_index,
                    line: 1,
                    detail: "Configured path match".to_owned(),
                    package_match: false,
                });
            }
        }
        for (specifier, line) in imports {
            for prefix in package_prefixes(specifier, package_separator) {
                let Some(rule_indices) = self.package_rule_indices.get(prefix) else {
                    continue;
                };
                for rule_index in rule_indices {
                    matches
                        .entry(*rule_index)
                        .and_modify(|current| {
                            if !current.package_match {
                                *current = CustomRuleMatch {
                                    rule_index: *rule_index,
                                    line,
                                    detail: format!("Configured package match: {specifier}"),
                                    package_match: true,
                                };
                            }
                        })
                        .or_insert_with(|| CustomRuleMatch {
                            rule_index: *rule_index,
                            line,
                            detail: format!("Configured package match: {specifier}"),
                            package_match: true,
                        });
                }
            }
        }
        matches.into_values().collect()
    }
}

fn package_prefixes(specifier: &str, separator: char) -> impl Iterator<Item = &str> {
    let mut prefixes = Vec::new();
    let mut candidate = specifier;
    loop {
        prefixes.push(candidate);
        let Some(index) = candidate.rfind(separator) else {
            break;
        };
        candidate = &candidate[..index];
    }
    prefixes.into_iter()
}

#[cfg(test)]
mod tests {
    use api_subway_core::{ApiSubwayConfig, DependencyKind, DependencyRule};

    use super::CompiledDependencyRules;

    #[test]
    fn matches_package_prefixes_and_paths_without_rule_cross_products() {
        let config = ApiSubwayConfig {
            dependencies: vec![DependencyRule {
                name: "Billing".to_owned(),
                kind: DependencyKind::Service,
                path_globs: vec!["src/billing/**".to_owned()],
                packages: vec!["@acme/billing".to_owned()],
                pin: false,
            }],
            ..ApiSubwayConfig::default()
        };
        let rules = CompiledDependencyRules::new(&config).expect("compiled rules");
        let package = rules.match_file("src/routes/orders.ts", [("@acme/billing/client", 7)], '/');
        assert_eq!(package.len(), 1);
        assert!(package[0].package_match);
        assert_eq!(package[0].line, 7);

        let path = rules.match_file("src/billing/handler.py", [], '.');
        assert_eq!(path.len(), 1);
        assert!(!path[0].package_match);
    }
}
