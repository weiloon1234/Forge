#[derive(forge::Model)]
#[forge(model = "users", primary_key_strategy = "manual", timestamps = false)]
struct User {
    id: i64,
    #[forge(read_accessor = "email_length")]
    email: String,
}

impl User {
    fn email_length(&self) -> usize {
        self.email.len()
    }
}

fn main() {}
