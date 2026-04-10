#![allow(unused_imports)]

use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "users", primary_key = "user_id")]
struct User {
    id: i64,
    email: String,
}

fn main() {}
