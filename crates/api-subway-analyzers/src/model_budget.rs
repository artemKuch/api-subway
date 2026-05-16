use std::io::{self, Write};

use api_subway_core::{ApiMapV1, Evidence};

const MAX_ENDPOINTS: usize = 10_000;
const MAX_SCHEMAS: usize = 50_000;
const MAX_DEPENDENCIES: usize = 10_000;
const MAX_RELATIONS: usize = 100_000;
const MAX_DIAGNOSTICS: usize = 50_000;
const MAX_NESTED_ITEMS: usize = 1_000;
const MAX_MODEL_JSON_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn validate(map: &ApiMapV1) -> Result<(), String> {
    check_count("endpoints", map.endpoints.len(), MAX_ENDPOINTS)?;
    check_count("schemas", map.schemas.len(), MAX_SCHEMAS)?;
    check_count("dependencies", map.dependencies.len(), MAX_DEPENDENCIES)?;
    check_count("relations", map.relations.len(), MAX_RELATIONS)?;
    check_count("diagnostics", map.diagnostics.len(), MAX_DIAGNOSTICS)?;
    check_text("project name", &map.project.name, 256)?;
    check_text("project root", &map.project.root, 4_096)?;
    for framework in &map.project.frameworks {
        check_text("framework name", framework, 64)?;
    }

    for endpoint in &map.endpoints {
        check_text("HTTP method", &endpoint.method, 16)?;
        check_text("endpoint path", &endpoint.path, 2_048)?;
        check_text("endpoint display path", &endpoint.display_path, 2_048)?;
        check_text("endpoint district", &endpoint.district, 256)?;
        check_text("endpoint framework", &endpoint.framework, 64)?;
        if let Some(operation_id) = &endpoint.operation_id {
            check_text("operation id", operation_id, 512)?;
        }
        check_count("endpoint tags", endpoint.tags.len(), MAX_NESTED_ITEMS)?;
        check_count("endpoint sources", endpoint.sources.len(), MAX_NESTED_ITEMS)?;
        for tag in &endpoint.tags {
            check_text("endpoint tag", tag, 256)?;
        }
        for source in &endpoint.sources {
            check_text("source path", &source.file, 4_096)?;
        }
        if let Some(contract) = &endpoint.contract {
            check_count(
                "request parameters",
                contract.request.parameters.len(),
                MAX_NESTED_ITEMS,
            )?;
            check_count(
                "request bodies",
                contract.request.bodies.len(),
                MAX_NESTED_ITEMS,
            )?;
            check_count("responses", contract.responses.len(), MAX_NESTED_ITEMS)?;
            check_count(
                "contract evidence",
                contract.evidence.len(),
                MAX_NESTED_ITEMS,
            )?;
            for response in &contract.responses {
                check_text("response status", &response.status, 7)?;
                check_count(
                    "response contents",
                    response.contents.len(),
                    MAX_NESTED_ITEMS,
                )?;
                for content in &response.contents {
                    check_text("response media type", &content.media_type, 256)?;
                }
            }
            for parameter in &contract.request.parameters {
                check_text("parameter name", &parameter.name, 512)?;
            }
            for content in &contract.request.bodies {
                check_text("request media type", &content.media_type, 256)?;
            }
            for evidence in &contract.evidence {
                check_evidence("contract evidence", evidence)?;
            }
        }
    }
    for schema in &map.schemas {
        if let Some(name) = &schema.name {
            check_text("schema name", name, 512)?;
        }
        if let Some(format) = &schema.format {
            check_text("schema format", format, 128)?;
        }
        check_count(
            "schema properties",
            schema.properties.len(),
            MAX_NESTED_ITEMS,
        )?;
        check_count("schema variants", schema.variants.len(), MAX_NESTED_ITEMS)?;
        check_count(
            "schema enum values",
            schema.enum_values.len(),
            MAX_NESTED_ITEMS,
        )?;
        check_count("schema evidence", schema.evidence.len(), MAX_NESTED_ITEMS)?;
        for property in &schema.properties {
            check_text("schema property name", &property.name, 512)?;
        }
        for value in &schema.enum_values {
            check_text("schema enum literal", &value.value, 4_096)?;
        }
        if let Some(value) = &schema.const_value {
            check_text("schema constant literal", &value.value, 4_096)?;
        }
        for evidence in &schema.evidence {
            check_evidence("schema evidence", evidence)?;
        }
    }
    for dependency in &map.dependencies {
        check_text("dependency name", &dependency.name, 512)?;
        check_count(
            "dependency packages",
            dependency.packages.len(),
            MAX_NESTED_ITEMS,
        )?;
        for package in &dependency.packages {
            check_text("dependency package", package, 512)?;
        }
    }
    for relation in &map.relations {
        check_count(
            "relation evidence",
            relation.evidence.len(),
            MAX_NESTED_ITEMS,
        )?;
        for evidence in &relation.evidence {
            check_evidence("relation evidence", evidence)?;
        }
    }
    for diagnostic in &map.diagnostics {
        check_text("diagnostic code", &diagnostic.code, 128)?;
        check_text("diagnostic message", &diagnostic.message, 4_096)?;
        if let Some(source) = &diagnostic.source {
            check_text("diagnostic source path", &source.file, 4_096)?;
        }
    }

    let mut writer = LimitedWriter::new(MAX_MODEL_JSON_BYTES);
    serde_json::to_writer(&mut writer, map)
        .map_err(|_| "serialized map exceeds the 32 MiB output budget".to_owned())?;
    Ok(())
}

fn check_evidence(name: &str, evidence: &Evidence) -> Result<(), String> {
    check_text(name, &evidence.detail, 4_096)?;
    if let Some(source) = &evidence.source {
        check_text("evidence source path", &source.file, 4_096)?;
    }
    Ok(())
}

fn check_text(name: &str, value: &str, maximum: usize) -> Result<(), String> {
    let actual = value.chars().count();
    if actual > maximum {
        return Err(format!(
            "{name} exceeds the supported text budget ({actual} > {maximum})"
        ));
    }
    Ok(())
}

fn check_count(name: &str, actual: usize, maximum: usize) -> Result<(), String> {
    if actual > maximum {
        return Err(format!(
            "{name} exceeds the supported item budget ({actual} > {maximum})"
        ));
    }
    Ok(())
}

struct LimitedWriter {
    written: usize,
    maximum: usize,
}

impl LimitedWriter {
    fn new(maximum: usize) -> Self {
        Self {
            written: 0,
            maximum,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(buffer.len()) > self.maximum {
            return Err(io::Error::other("serialized output budget exceeded"));
        }
        self.written += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{LimitedWriter, check_count, check_text};

    #[test]
    fn rejects_collection_and_serialized_size_overflow() {
        assert!(check_count("items", 2, 2).is_ok());
        assert!(check_count("items", 3, 2).is_err());
        assert!(check_text("name", "é", 1).is_ok());
        assert!(check_text("name", "too long", 3).is_err());

        let mut writer = LimitedWriter::new(4);
        assert!(writer.write_all(b"1234").is_ok());
        assert!(writer.write_all(b"5").is_err());
    }
}
