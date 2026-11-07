//! Snowflake IDs: 64-bit, roughly time-ordered, unique per server.
//!
//! Layout (like Discord's): 42 bits of milliseconds since [`EPOCH_MS`],
//! 5 bits of worker id, 5 unused bits, 12 bits of per-millisecond sequence.
//! IDs go over JSON as strings, because JavaScript numbers lose precision
//! above 2^53.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 2026-01-01T00:00:00Z in milliseconds since the Unix epoch.
pub const EPOCH_MS: u64 = 1_767_225_600_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_millis() as u64
}

/// Generates unique snowflake IDs. Cheap to share behind an `Arc`.
pub struct SnowflakeGen {
    worker: u64,
    /// (last timestamp in ms, sequence within that ms)
    state: Mutex<(u64, u64)>,
}

impl SnowflakeGen {
    pub fn new(worker: u64) -> Self {
        Self {
            worker: worker & 0x1F,
            state: Mutex::new((0, 0)),
        }
    }

    pub fn next_id(&self) -> i64 {
        let mut st = self.state.lock().expect("snowflake mutex poisoned");
        let mut now = now_ms().max(st.0);
        if now == st.0 {
            st.1 = (st.1 + 1) & 0xFFF;
            if st.1 == 0 {
                // 4096 IDs in one millisecond: wait for the next one.
                while now <= st.0 {
                    std::hint::spin_loop();
                    now = now_ms();
                }
            }
        } else {
            st.1 = 0;
        }
        st.0 = now;
        (((now - EPOCH_MS) << 22) | (self.worker << 17) | st.1) as i64
    }
}

/// When a snowflake ID was created.
pub fn created_at_ms(id: i64) -> u64 {
    ((id as u64) >> 22) + EPOCH_MS
}

/// `#[serde(with = "as_string")]` for `i64` IDs.
pub mod as_string {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(i64),
        }
        match Raw::deserialize(d)? {
            Raw::S(s) => s.parse().map_err(D::Error::custom),
            Raw::N(n) => Ok(n),
        }
    }
}

/// `#[serde(with = "vec_as_string")]` for `Vec<i64>` IDs.
pub mod vec_as_string {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[i64], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(v.iter().map(|id| id.to_string()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<i64>, D::Error> {
        let raw = Vec::<String>::deserialize(d)?;
        raw.iter()
            .map(|s| s.parse().map_err(D::Error::custom))
            .collect()
    }
}

/// `#[serde(with = "option_as_string")]` for `Option<i64>` IDs.
pub mod option_as_string {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(id) => s.serialize_some(&id.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        let raw = Option::<String>::deserialize(d)?;
        raw.map(|s| s.parse().map_err(D::Error::custom)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_increasing() {
        let g = SnowflakeGen::new(1);
        let mut last = 0;
        for _ in 0..10_000 {
            let id = g.next_id();
            assert!(id > last);
            last = id;
        }
    }

    #[test]
    fn timestamp_round_trips() {
        let g = SnowflakeGen::new(0);
        let before = now_ms();
        let id = g.next_id();
        let ts = created_at_ms(id);
        assert!(ts >= before && ts <= now_ms());
    }
}
