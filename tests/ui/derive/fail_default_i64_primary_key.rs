#[derive(forge::Model)]
#[forge(model = "users")]
struct User {
    id: i64,
    email: String,
}

fn main() {}
