use sea_orm::Database;
use site::migration::{Migrator, MigratorTrait};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = Database::connect(&database_url)
        .await
        .expect("Failed to connect to database");

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("up");

    match command {
        "up" => {
            Migrator::up(&db, None).await.expect("Migration failed");
            println!("Migrations applied successfully.");
        }
        "down" => {
            Migrator::down(&db, None).await.expect("Rollback failed");
            println!("Last migration rolled back.");
        }
        "fresh" => {
            Migrator::fresh(&db).await.expect("Fresh migration failed");
            println!("Database reset and migrations re-applied.");
        }
        "status" => {
            Migrator::status(&db).await.expect("Status check failed");
        }
        _ => {
            eprintln!("Usage: site_migration [up|down|fresh|status]");
            std::process::exit(1);
        }
    }
}
