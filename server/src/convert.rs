//! Conversions between database values and protobuf types, shared by all services.

use std::time::SystemTime;

use buffa_types::google::protobuf::Timestamp;
use chrono::{DateTime, Utc};

use crate::proto::thehideout::v1::User;
use crate::snowflake;

/// Reinterprets a Snowflake as the signed `bigint` CQL stores it in.
pub fn id_to_db(id: u64) -> i64 {
    id as i64
}

/// Reinterprets a CQL `bigint` as the Snowflake it was stored from.
pub fn id_from_db(id: i64) -> u64 {
    id as u64
}

pub fn system_time_to_proto(t: SystemTime) -> Timestamp {
    Timestamp::from(t)
}

pub fn datetime_to_proto(t: DateTime<Utc>) -> Timestamp {
    system_time_to_proto(t.into())
}

pub fn proto_to_system_time(ts: &Timestamp) -> Option<SystemTime> {
    SystemTime::try_from(ts.clone()).ok()
}

pub fn proto_to_datetime(ts: &Timestamp) -> Option<DateTime<Utc>> {
    proto_to_system_time(ts).map(DateTime::from)
}

/// Creation time encoded in a Snowflake, as a database `timestamp` value.
pub fn snowflake_datetime(id: u64) -> DateTime<Utc> {
    snowflake::timestamp_of(id).into()
}

pub fn user_to_proto(id: u64, username: &str, created_at: DateTime<Utc>) -> User {
    User {
        id,
        username: username.to_owned(),
        created_at: datetime_to_proto(created_at).into(),
        ..Default::default()
    }
}
