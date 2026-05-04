#[derive(forge::Model)]
#[forge(table = "users")]
struct User {
    id: i64,
    email: String,
}

fn main() {}
