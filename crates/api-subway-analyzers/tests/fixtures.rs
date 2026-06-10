use std::collections::BTreeMap;
use std::path::PathBuf;
use std::{fs, path::Path, str::FromStr};

use api_subway_analyzers::{AnalyzeOptions, Framework, analyze};
use api_subway_core::{ApiMapV1, Confidence, DependencyKind};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join(name)
}

fn write_fixture(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(path, contents).expect("fixture source");
}

#[test]
fn matches_the_committed_accuracy_corpus() {
    let corpus_root = fixture("corpus");
    let manifest_path = corpus_root.join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(&manifest_path).expect("accuracy corpus manifest should be readable"),
    )
    .expect("accuracy corpus manifest should be valid JSON");
    assert_eq!(manifest["schema_version"].as_u64(), Some(1));
    let cases = manifest["cases"]
        .as_array()
        .expect("accuracy corpus cases should be an array");

    for case in cases {
        let name = case["name"]
            .as_str()
            .expect("accuracy case should have a name");
        let relative_root = case["root"]
            .as_str()
            .expect("accuracy case should have a root");
        let framework = Framework::from_str(
            case["framework"]
                .as_str()
                .expect("accuracy case should have a framework"),
        )
        .expect("accuracy case framework should be supported");
        let mut options = AnalyzeOptions::new(corpus_root.join(relative_root));
        options.frameworks = vec![framework];
        let map = analyze(&options).unwrap_or_else(|error| panic!("{name}: {error}"));

        assert!(
            map.diagnostics.is_empty(),
            "{name}: expected no diagnostics, got {:?}",
            map.diagnostics
        );
        let mut actual_endpoints = map
            .endpoints
            .iter()
            .map(|endpoint| format!("{} {}", endpoint.method, endpoint.path))
            .collect::<Vec<_>>();
        actual_endpoints.sort();
        let expected_endpoints = string_array(&case["endpoints"], "case endpoints");
        assert_eq!(actual_endpoints, expected_endpoints, "{name}: endpoints");

        let actual_relations = map
            .dependencies
            .iter()
            .map(|dependency| {
                let kind = serde_json::to_value(dependency.kind)
                    .expect("dependency kind should serialize")
                    .as_str()
                    .expect("dependency kind should serialize as a string")
                    .to_owned();
                (
                    format!("{kind}:{}", dependency.name),
                    relation_endpoints(&map, &dependency.id),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let expected_relations = case["relations"]
            .as_object()
            .expect("case relations should be an object")
            .iter()
            .map(|(dependency, endpoints)| {
                (
                    dependency.clone(),
                    string_array(endpoints, "relation endpoints"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(actual_relations, expected_relations, "{name}: relations");
    }
}

fn string_array(value: &serde_json::Value, field: &str) -> Vec<String> {
    let mut values = value
        .as_array()
        .unwrap_or_else(|| panic!("{field} should be an array"))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("{field} entries should be strings"))
                .to_owned()
        })
        .collect::<Vec<_>>();
    values.sort();
    values
}

#[test]
fn extracts_next_app_router_routes_and_proxy() {
    let mut options = AnalyzeOptions::new(fixture("next"));
    options.frameworks = vec![Framework::Next];
    let map = analyze(&options).expect("Next fixture should analyze");
    let stations = map
        .endpoints
        .iter()
        .map(|endpoint| format!("{} {}", endpoint.method, endpoint.path))
        .collect::<Vec<_>>();
    assert_eq!(
        stations,
        [
            "GET /api/users",
            "POST /api/users",
            "GET /api/users/{id}",
            "PATCH /api/users/{id}",
            "GET /docs/{slug?*}",
        ]
    );
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "Next Proxy")
    );
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "Prisma")
    );
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "OpenAI")
    );
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "Users")
    );
    assert!(
        !map.dependencies
            .iter()
            .any(|dependency| dependency.name == "Stripe"),
        "an unused package import must not become a line"
    );
    let user_service_relations = relations_for(&map, "Users");
    assert_eq!(user_service_relations.len(), 1);
    assert_eq!(user_service_relations[0].confidence, Confidence::Inferred);
    assert_eq!(
        endpoint(&map, &user_service_relations[0].endpoint_id),
        "GET /api/users/{id}"
    );
    let openai_relations = relations_for(&map, "OpenAI");
    assert_eq!(openai_relations.len(), 1);
    assert!(openai_relations[0].evidence.iter().any(|evidence| {
        evidence
            .source
            .as_ref()
            .is_some_and(|source| source.file == "src/services/users.ts" && source.line == 16)
    }));
    let create_user = map
        .endpoints
        .iter()
        .find(|endpoint| endpoint.method == "POST" && endpoint.path == "/api/users")
        .expect("POST user station");
    let contract = create_user.contract.as_ref().expect("Zod contract");
    assert_eq!(contract.request.bodies.len(), 1);
    assert_eq!(contract.responses[0].status, "201");
    assert!(!map.schemas.is_empty());
}

#[test]
fn resolves_express_mounts_route_chains_and_middleware_order() {
    let mut options = AnalyzeOptions::new(fixture("express"));
    options.frameworks = vec![Framework::Express];
    let map = analyze(&options).expect("Express fixture should analyze");
    let stations = map
        .endpoints
        .iter()
        .map(|endpoint| format!("{} {}", endpoint.method, endpoint.path))
        .collect::<Vec<_>>();
    assert!(stations.contains(&"GET /api/users".to_owned()));
    assert!(stations.contains(&"GET /api/users/{id}".to_owned()));
    assert!(stations.contains(&"PATCH /api/users/{id}".to_owned()));
    assert!(stations.contains(&"GET /health".to_owned()));
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "Stripe")
    );
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "authorize")
    );
    assert!(
        !map.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "express-dynamic-path")
    );
    let stripe = map
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Stripe")
        .expect("Stripe dependency");
    let stripe_endpoints = map
        .relations
        .iter()
        .filter(|relation| relation.dependency_id == stripe.id)
        .map(|relation| relation.endpoint_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        stripe_endpoints.len(),
        1,
        "handler-scoped analysis must not leak Stripe to unrelated routes"
    );
    assert!(
        map.relations
            .iter()
            .filter(|relation| relation.dependency_id == stripe.id)
            .flat_map(|relation| &relation.evidence)
            .any(|evidence| evidence.source.as_ref().is_some_and(|source| {
                source.file == "src/routes/users.ts" && source.line == 11
            })),
        "package evidence must point at the call, not the import"
    );
    let update_user = map
        .endpoints
        .iter()
        .find(|endpoint| endpoint.method == "PATCH")
        .expect("PATCH user station");
    let contract = update_user.contract.as_ref().expect("Zod contract");
    assert_eq!(contract.request.bodies.len(), 1);
    assert_eq!(contract.responses[0].status, "200");
    assert_eq!(
        contract
            .request
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>(),
        ["id"]
    );
}

#[test]
fn express_ignores_unrelated_method_calls_and_does_not_guess_dynamic_mounts() {
    let temporary = tempfile::tempdir().expect("temporary fixture");
    write_fixture(
        temporary.path(),
        "package.json",
        r#"{"dependencies":{"express":"5","axios":"1"}}"#,
    );
    write_fixture(
        temporary.path(),
        "app.ts",
        r#"
import express from "express";
import axios from "axios";
import users from "./users";

const app = express();
const cache = new Map<string, string>();
const API_PREFIX = process.env.API_PREFIX;

axios.get("/remote");
cache.get("/cached");
app.use(API_PREFIX, users);
app.get("/health", health);

function health() {}
"#,
    );
    write_fixture(
        temporary.path(),
        "users.ts",
        r#"
import { Router } from "express";

const router = Router();
router.get("/users", listUsers);
export default router;

function listUsers() {}
"#,
    );

    let mut options = AnalyzeOptions::new(temporary.path());
    options.frameworks = vec![Framework::Express];
    let map = analyze(&options).expect("Express fixture should analyze");
    let stations = map
        .endpoints
        .iter()
        .map(|endpoint| format!("{} {}", endpoint.method, endpoint.path))
        .collect::<Vec<_>>();

    assert_eq!(stations, ["GET /health"]);
    assert!(map.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "express-dynamic-mount"
            && diagnostic
                .source
                .as_ref()
                .is_some_and(|source| source.file == "app.ts")
    }));
    assert!(
        !map.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "express-dynamic-path"),
        "axios.get and Map.get must not be parsed as Express routes"
    );
}

#[test]
fn express_resolves_static_mount_constants() {
    let temporary = tempfile::tempdir().expect("temporary fixture");
    write_fixture(
        temporary.path(),
        "app.ts",
        r#"
import express from "express";
import users from "./users";

const API_PREFIX = "/api";
const app = express();
app.use(API_PREFIX, users);
"#,
    );
    write_fixture(
        temporary.path(),
        "users.ts",
        r#"
import { Router } from "express";
const router = Router();
router.get("/users", listUsers);
export default router;
function listUsers() {}
"#,
    );

    let mut options = AnalyzeOptions::new(temporary.path());
    options.frameworks = vec![Framework::Express];
    let map = analyze(&options).expect("Express fixture should analyze");

    assert!(
        map.endpoints
            .iter()
            .any(|endpoint| { endpoint.method == "GET" && endpoint.path == "/api/users" })
    );
    assert!(
        !map.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "express-dynamic-mount")
    );
}

#[test]
fn javascript_call_graph_follows_imported_handlers_and_barrel_reexports() {
    let temporary = tempfile::tempdir().expect("temporary fixture");
    write_fixture(
        temporary.path(),
        "app.ts",
        r#"
import express from "express";
import { createOrder } from "./handlers";

const app = express();
app.post("/orders", createOrder);
"#,
    );
    write_fixture(
        temporary.path(),
        "handlers/index.ts",
        "export { createOrder } from \"./orders\";\n",
    );
    write_fixture(
        temporary.path(),
        "handlers/orders.ts",
        r#"
import Stripe from "stripe";

export function createOrder() {
  return Stripe.paymentIntents.create({ amount: 1000, currency: "usd" });
}
"#,
    );

    let mut options = AnalyzeOptions::new(temporary.path());
    options.frameworks = vec![Framework::Express];
    let map = analyze(&options).expect("barrel fixture should analyze");

    assert_eq!(
        relation_endpoints(
            &map,
            &dependency(&map, "Stripe", DependencyKind::External).id
        ),
        ["POST /orders"]
    );
}

#[test]
fn resolves_fastapi_prefixes_annotated_and_subdependencies() {
    let mut options = AnalyzeOptions::new(fixture("fastapi"));
    options.frameworks = vec![Framework::FastApi];
    let map = analyze(&options).expect("FastAPI fixture should analyze");
    let stations = map
        .endpoints
        .iter()
        .map(|endpoint| format!("{} {}", endpoint.method, endpoint.path))
        .collect::<Vec<_>>();
    assert!(stations.contains(&"GET /api/items/{item_id}".to_owned()));
    assert!(stations.contains(&"POST /api/items/sync".to_owned()));
    assert!(stations.contains(&"PUT /api/items/sync".to_owned()));
    assert!(stations.contains(&"GET /health".to_owned()));
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "SQLAlchemy")
    );
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "session_scope")
    );
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "Catalog")
    );
    assert!(
        !map.dependencies
            .iter()
            .any(|dependency| dependency.name == "HTTP client"),
        "an unused HTTP client import must not become a line"
    );
    let stripe = map
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Stripe")
        .expect("Stripe dependency");
    assert_eq!(
        map.relations
            .iter()
            .filter(|relation| relation.dependency_id == stripe.id)
            .count(),
        2,
        "Stripe should connect only the POST and PUT sync operations"
    );
    assert!(
        map.relations
            .iter()
            .filter(|relation| relation.dependency_id == stripe.id)
            .flat_map(|relation| &relation.evidence)
            .any(|evidence| evidence.source.as_ref().is_some_and(|source| {
                source.file == "app/services/catalog.py" && source.line == 12
            }))
    );
    let sync = map
        .endpoints
        .iter()
        .find(|endpoint| endpoint.method == "POST")
        .expect("POST sync station");
    let contract = sync.contract.as_ref().expect("Pydantic contract");
    assert_eq!(contract.request.bodies.len(), 1);
    assert_eq!(contract.responses[0].status, "201");
}

#[test]
fn fastapi_keeps_same_named_subdependencies_module_scoped() {
    let temporary = tempfile::tempdir().expect("temporary fixture");
    write_fixture(
        temporary.path(),
        "main.py",
        r"
from fastapi import FastAPI
from routes_a import router as router_a
from routes_b import router as router_b

app = FastAPI()
app.include_router(router_a)
app.include_router(router_b)
",
    );
    write_fixture(
        temporary.path(),
        "routes_a.py",
        r#"
from fastapi import APIRouter, Depends

router = APIRouter()

def only_a():
    return "a"

def shared_dependency(value=Depends(only_a)):
    return value

@router.get("/a")
def route_a(value=Depends(shared_dependency)):
    return value
"#,
    );
    write_fixture(
        temporary.path(),
        "routes_b.py",
        r#"
from fastapi import APIRouter, Depends

router = APIRouter()

def only_b():
    return "b"

def shared_dependency(value=Depends(only_b)):
    return value

@router.get("/b")
def route_b(value=Depends(shared_dependency)):
    return value
"#,
    );

    let mut options = AnalyzeOptions::new(temporary.path());
    options.frameworks = vec![Framework::FastApi];
    let map = analyze(&options).expect("FastAPI fixture should analyze");

    assert_eq!(
        relation_endpoints(
            &map,
            &dependency(&map, "only_a", DependencyKind::Middleware).id
        ),
        ["GET /a"]
    );
    assert_eq!(
        relation_endpoints(
            &map,
            &dependency(&map, "only_b", DependencyKind::Middleware).id
        ),
        ["GET /b"]
    );
}

#[test]
fn fastapi_resolves_absolute_imports_from_a_src_layout() {
    let temporary = tempfile::tempdir().expect("temporary fixture");
    write_fixture(
        temporary.path(),
        "pyproject.toml",
        "[project]\nname = \"src-layout\"\ndependencies = [\"fastapi\"]\n",
    );
    write_fixture(
        temporary.path(),
        "src/app/main.py",
        r#"
from fastapi import FastAPI
from app.routes import router

app = FastAPI()
app.include_router(router, prefix="/api")
"#,
    );
    write_fixture(
        temporary.path(),
        "src/app/routes.py",
        r#"
from fastapi import APIRouter

router = APIRouter()

@router.get("/users")
def list_users():
    return []
"#,
    );

    let mut options = AnalyzeOptions::new(temporary.path());
    options.frameworks = vec![Framework::FastApi];
    let map = analyze(&options).expect("FastAPI src-layout fixture should analyze");

    assert!(
        map.endpoints
            .iter()
            .any(|endpoint| { endpoint.method == "GET" && endpoint.path == "/api/users" })
    );
    assert!(
        !map.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "fastapi-unresolved-router")
    );
}

#[test]
fn python_call_graph_follows_package_reexports() {
    let temporary = tempfile::tempdir().expect("temporary fixture");
    write_fixture(
        temporary.path(),
        "main.py",
        r#"
from fastapi import FastAPI
from services import create_payment

app = FastAPI()

@app.post("/payments")
def payment():
    return create_payment()
"#,
    );
    write_fixture(
        temporary.path(),
        "services/__init__.py",
        "from .payments import create_payment\n",
    );
    write_fixture(
        temporary.path(),
        "services/payments.py",
        r#"
import stripe

def create_payment():
    return stripe.PaymentIntent.create(amount=1000, currency="usd")
"#,
    );

    let mut options = AnalyzeOptions::new(temporary.path());
    options.frameworks = vec![Framework::FastApi];
    let map = analyze(&options).expect("Python package fixture should analyze");

    assert_eq!(
        relation_endpoints(
            &map,
            &dependency(&map, "Stripe", DependencyKind::External).id
        ),
        ["POST /payments"]
    );
}

#[test]
fn reports_when_no_supported_routes_are_found() {
    let temporary = tempfile::tempdir().expect("temporary fixture");
    write_fixture(temporary.path(), "index.ts", "export const value = 42;\n");

    let map = analyze(&AnalyzeOptions::new(temporary.path())).expect("empty project analysis");

    assert!(map.endpoints.is_empty());
    assert!(map.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "no-routes"
            && diagnostic.severity == api_subway_core::DiagnosticSeverity::Warning
    }));
}

fn relations_for<'a>(
    map: &'a ApiMapV1,
    dependency_name: &str,
) -> Vec<&'a api_subway_core::Relation> {
    let dependency = map
        .dependencies
        .iter()
        .find(|dependency| dependency.name == dependency_name)
        .expect("dependency exists");
    map.relations
        .iter()
        .filter(|relation| relation.dependency_id == dependency.id)
        .collect()
}

fn endpoint(map: &ApiMapV1, endpoint_id: &str) -> String {
    let endpoint = map
        .endpoints
        .iter()
        .find(|endpoint| endpoint.id == endpoint_id)
        .expect("endpoint exists");
    format!("{} {}", endpoint.method, endpoint.path)
}

#[test]
fn traces_local_boundaries_shared_helpers_and_computed_calls() {
    let mut options = AnalyzeOptions::new(fixture("dependencies"));
    options.frameworks = vec![Framework::Express];
    let map = analyze(&options).expect("dependency fixture should analyze");

    assert!(
        !map.dependencies
            .iter()
            .any(|dependency| dependency.name == "Stripe"),
        "unused imports must not create dependencies"
    );
    let orders_service = dependency(&map, "Orders", DependencyKind::Service);
    let orders_repository = dependency(&map, "Orders", DependencyKind::Datastore);
    let postgres = dependency(&map, "Postgres client", DependencyKind::Datastore);
    let notifications = dependency(&map, "Notifications", DependencyKind::External);
    let twilio = dependency(&map, "Twilio", DependencyKind::External);

    assert_eq!(
        relation_endpoints(&map, &orders_service.id),
        [
            "GET /api/orders",
            "PATCH /api/orders/{id}",
            "POST /api/orders"
        ]
    );
    assert_eq!(
        relation_endpoints(&map, &orders_repository.id),
        ["GET /api/orders", "POST /api/orders"]
    );
    assert_eq!(
        relation_endpoints(&map, &postgres.id),
        ["GET /api/orders", "POST /api/orders"]
    );
    assert_eq!(
        relation_endpoints(&map, &notifications.id),
        ["POST /api/orders"]
    );
    assert_eq!(relation_endpoints(&map, &twilio.id), ["POST /api/orders"]);

    let postgres_relation = map
        .relations
        .iter()
        .find(|relation| {
            relation.dependency_id == postgres.id
                && endpoint(&map, &relation.endpoint_id) == "POST /api/orders"
        })
        .expect("POST to Postgres relation");
    assert!(postgres_relation.evidence.iter().any(|evidence| {
        evidence
            .detail
            .contains("local module src/services/orders.ts")
    }));
    assert!(postgres_relation.evidence.iter().any(|evidence| {
        evidence
            .detail
            .contains("local module src/repositories/orders.ts")
    }));
    assert!(postgres_relation.evidence.iter().any(|evidence| {
        evidence
            .source
            .as_ref()
            .is_some_and(|source| source.file == "src/repositories/orders.ts" && source.line == 10)
    }));
    assert!(map.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "dynamic-dependency-call"
            && diagnostic
                .source
                .as_ref()
                .is_some_and(|source| source.file == "src/routes/orders.ts" && source.line == 17)
    }));
}

fn dependency<'a>(
    map: &'a ApiMapV1,
    name: &str,
    kind: DependencyKind,
) -> &'a api_subway_core::Dependency {
    map.dependencies
        .iter()
        .find(|dependency| dependency.name == name && dependency.kind == kind)
        .expect("dependency exists")
}

fn relation_endpoints(map: &ApiMapV1, dependency_id: &str) -> Vec<String> {
    let mut endpoints = map
        .relations
        .iter()
        .filter(|relation| relation.dependency_id == dependency_id)
        .map(|relation| endpoint(map, &relation.endpoint_id))
        .collect::<Vec<_>>();
    endpoints.sort();
    endpoints
}

#[test]
fn builds_the_committed_demo_from_real_source_and_openapi() {
    let root = fixture("demo");
    let mut options = AnalyzeOptions::new(&root);
    options.frameworks = vec![Framework::Express];
    options.openapi = vec![PathBuf::from("openapi.yaml")];
    let map = analyze(&options).expect("demo fixture should analyze");

    assert_eq!(map.endpoints.len(), 10);
    assert_eq!(map.dependencies.len(), 18);
    assert!(map.diagnostics.is_empty());
    assert!(
        map.endpoints
            .iter()
            .all(|endpoint| endpoint.contract.is_some())
    );
    assert!(
        map.relations
            .iter()
            .flat_map(|relation| &relation.evidence)
            .all(|evidence| !evidence.detail.contains("Synthetic"))
    );
    assert_eq!(
        relation_endpoints(
            &map,
            &dependency(&map, "Stripe", DependencyKind::External).id
        )
        .len(),
        3
    );
    assert_eq!(
        relation_endpoints(
            &map,
            &dependency(&map, "Prisma", DependencyKind::Datastore).id
        )
        .len(),
        3
    );
}

#[test]
fn merges_openapi_metadata_and_spec_only_operations() {
    let mut options = AnalyzeOptions::new(fixture("next"));
    options.frameworks = vec![Framework::Next];
    options.openapi = vec![PathBuf::from("../openapi/api.yaml")];
    let error = analyze(&options).expect_err("OpenAPI symlink/root boundary must be enforced");
    assert!(error.to_string().contains("outside the analysis root"));

    let root = fixture("openapi");
    let mut options = AnalyzeOptions::new(&root);
    options.frameworks = Vec::new();
    options.openapi = vec![PathBuf::from("api.yaml")];
    let map = analyze(&options).expect("OpenAPI fixture should analyze");
    assert_eq!(map.endpoints.len(), 2);
    assert!(map.endpoints.iter().all(|endpoint| endpoint.spec_only));
    let get_user = map
        .endpoints
        .iter()
        .find(|endpoint| endpoint.operation_id.as_deref() == Some("getUser"))
        .expect("getUser operation");
    assert_eq!(get_user.sources[0].line, 9);
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.name == "bearerAuth")
    );
    let json = serde_json::to_string(&map).expect("map serializes");
    assert!(!json.contains("This description must never"));
    assert!(!json.contains("example-secret"));
    assert!(
        map.endpoints
            .iter()
            .all(|endpoint| endpoint.contract.is_some())
    );
    assert!(
        map.schemas
            .iter()
            .any(|schema| !schema.properties.is_empty())
    );
}
