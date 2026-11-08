use serde::{Deserialize, Serialize};
use ts_rs::TS;

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// A UUIDv7 (time-ordered) rendered as a string.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
        #[ts(export, type = "string")]
        pub struct $name(pub String);

        impl $name {
            pub fn generate() -> Self {
                Self(uuid::Uuid::now_v7().to_string())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    };
}

id_type!(ProjectId, "A project: one repository (or directory) added to Atelier.");
id_type!(SessionId, "An agent session. For new Claude Code sessions this is also the CLI's session id.");
