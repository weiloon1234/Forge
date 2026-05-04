#[derive(Clone, Copy, Debug, PartialEq, Eq, forge::AppEnum)]
#[forge(id_type = forge::PermissionId)]
enum Status {
    Draft = 1,
    Published = 2,
}

fn main() {}
