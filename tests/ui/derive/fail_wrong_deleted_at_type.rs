use forge::prelude::*;

#[derive(forge::Model)]
#[forge(model = "posts", soft_deletes = true)]
struct Post {
    id: ModelId<Post>,
    title: String,
    created_at: DateTime,
    updated_at: DateTime,
    deleted_at: DateTime,
}

fn main() {}
