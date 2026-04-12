#![allow(unused_imports)]

use forge::prelude::*;

#[derive(Clone, Debug, PartialEq)]
struct CustomType;

#[derive(forge::Model)]
#[forge(model = "users", primary_key_strategy = "manual")]
struct User {
    id: i64,
    custom: CustomType,
}

fn main() {}
