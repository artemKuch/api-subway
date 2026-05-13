use api_subway_core::DependencyKind;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CatalogEntry {
    pub name: &'static str,
    pub kind: DependencyKind,
    pub packages: &'static [&'static str],
}

pub(crate) const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        name: "Prisma",
        kind: DependencyKind::Datastore,
        packages: &["@prisma/client"],
    },
    CatalogEntry {
        name: "Drizzle",
        kind: DependencyKind::Datastore,
        packages: &["drizzle-orm"],
    },
    CatalogEntry {
        name: "TypeORM",
        kind: DependencyKind::Datastore,
        packages: &["typeorm"],
    },
    CatalogEntry {
        name: "Sequelize",
        kind: DependencyKind::Datastore,
        packages: &["sequelize"],
    },
    CatalogEntry {
        name: "Mongoose",
        kind: DependencyKind::Datastore,
        packages: &["mongoose"],
    },
    CatalogEntry {
        name: "Postgres client",
        kind: DependencyKind::Datastore,
        packages: &["pg", "asyncpg", "psycopg", "psycopg2"],
    },
    CatalogEntry {
        name: "MySQL client",
        kind: DependencyKind::Datastore,
        packages: &["mysql2"],
    },
    CatalogEntry {
        name: "Redis",
        kind: DependencyKind::Datastore,
        packages: &["redis", "ioredis"],
    },
    CatalogEntry {
        name: "SQLAlchemy",
        kind: DependencyKind::Datastore,
        packages: &["sqlalchemy"],
    },
    CatalogEntry {
        name: "SQLModel",
        kind: DependencyKind::Datastore,
        packages: &["sqlmodel"],
    },
    CatalogEntry {
        name: "PyMongo",
        kind: DependencyKind::Datastore,
        packages: &["pymongo"],
    },
    CatalogEntry {
        name: "MongoDB",
        kind: DependencyKind::Datastore,
        packages: &["mongodb"],
    },
    CatalogEntry {
        name: "SQLite",
        kind: DependencyKind::Datastore,
        packages: &["better-sqlite3", "aiosqlite"],
    },
    CatalogEntry {
        name: "Knex",
        kind: DependencyKind::Datastore,
        packages: &["knex"],
    },
    CatalogEntry {
        name: "Kysely",
        kind: DependencyKind::Datastore,
        packages: &["kysely"],
    },
    CatalogEntry {
        name: "Supabase",
        kind: DependencyKind::Datastore,
        packages: &["@supabase/supabase-js", "supabase"],
    },
    CatalogEntry {
        name: "Firebase",
        kind: DependencyKind::Datastore,
        packages: &["firebase-admin", "firebase_admin"],
    },
    CatalogEntry {
        name: "Elasticsearch",
        kind: DependencyKind::Datastore,
        packages: &["@elastic/elasticsearch", "elasticsearch"],
    },
    CatalogEntry {
        name: "Stripe",
        kind: DependencyKind::External,
        packages: &["stripe"],
    },
    CatalogEntry {
        name: "AWS SDK",
        kind: DependencyKind::External,
        packages: &["aws-sdk", "@aws-sdk/", "boto3"],
    },
    CatalogEntry {
        name: "Twilio",
        kind: DependencyKind::External,
        packages: &["twilio"],
    },
    CatalogEntry {
        name: "SendGrid",
        kind: DependencyKind::External,
        packages: &["@sendgrid/mail", "sendgrid"],
    },
    CatalogEntry {
        name: "OpenAI",
        kind: DependencyKind::External,
        packages: &["openai"],
    },
    CatalogEntry {
        name: "HTTP client",
        kind: DependencyKind::External,
        packages: &[
            "axios",
            "got",
            "undici",
            "node-fetch",
            "ky",
            "requests",
            "httpx",
            "aiohttp",
        ],
    },
    CatalogEntry {
        name: "Resend",
        kind: DependencyKind::External,
        packages: &["resend"],
    },
    CatalogEntry {
        name: "Postmark",
        kind: DependencyKind::External,
        packages: &["postmark"],
    },
    CatalogEntry {
        name: "Cloudinary",
        kind: DependencyKind::External,
        packages: &["cloudinary"],
    },
    CatalogEntry {
        name: "Algolia",
        kind: DependencyKind::External,
        packages: &["algoliasearch"],
    },
];

pub(crate) fn by_package(package: &str) -> Option<&'static CatalogEntry> {
    let root = package.split('.').next().unwrap_or(package);
    CATALOG.iter().find(|entry| {
        entry.packages.iter().any(|candidate| {
            if candidate.ends_with('/') {
                package.starts_with(candidate)
            } else {
                package == *candidate
                    || package.starts_with(&format!("{candidate}/"))
                    || root == *candidate
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::by_package;

    #[test]
    fn matches_javascript_subpaths_and_python_modules_without_prefix_collisions() {
        assert_eq!(
            by_package("@aws-sdk/client-s3").map(|entry| entry.name),
            Some("AWS SDK")
        );
        assert_eq!(
            by_package("sqlalchemy.orm").map(|entry| entry.name),
            Some("SQLAlchemy")
        );
        assert_eq!(
            by_package("@prisma/client/runtime").map(|entry| entry.name),
            Some("Prisma")
        );
        assert!(by_package("stripe-mock").is_none());
    }
}
