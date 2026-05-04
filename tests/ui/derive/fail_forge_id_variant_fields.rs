use forge::ForgeId;

#[derive(ForgeId)]
#[forge(id = forge::GuardId, rename_all = "snake_case")]
enum Guard {
    Api(String),
}

fn main() {}
