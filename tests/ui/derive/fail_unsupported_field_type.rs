#![allow(unused_imports)]

use forge::prelude::*;

#[derive(Clone, Debug, PartialEq)]
struct CustomType;

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "users")]
struct User {
    id: i64,
    custom: CustomType,
}

fn main() {}
