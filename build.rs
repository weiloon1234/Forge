fn main() -> std::io::Result<()> {
    forge_build::DatabaseCodegen::new()
        .migration_dir("database/migrations")
        .seeder_dir("database/seeders")
        .generate()
}
