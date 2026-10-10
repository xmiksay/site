use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str());

    match command {
        Some("create-user") => {
            if args.len() < 4 {
                eprintln!("Usage: site_cli create-user <username> <password>");
                std::process::exit(1);
            }
            let (username, password) = (&args[2], &args[3]);

            let db = Database::connect(std::env::var("DATABASE_URL").unwrap())
                .await
                .unwrap();

            let hash = site::auth::hash_password(password);

            site::entity::user::ActiveModel {
                username: Set(username.clone()),
                password_hash: Set(hash),
                ..Default::default()
            }
            .insert(&db)
            .await
            .unwrap();

            println!("User '{username}' created.");
        }
        Some("change-password") => {
            if args.len() < 4 {
                eprintln!("Usage: site_cli change-password <username> <new_password>");
                std::process::exit(1);
            }
            let (username, password) = (&args[2], &args[3]);

            let db = Database::connect(std::env::var("DATABASE_URL").unwrap())
                .await
                .unwrap();

            let user = site::entity::user::Entity::find()
                .filter(site::entity::user::Column::Username.eq(username.as_str()))
                .one(&db)
                .await
                .unwrap()
                .unwrap_or_else(|| {
                    eprintln!("User '{username}' not found.");
                    std::process::exit(1);
                });

            let hash = site::auth::hash_password(password);
            let mut active: site::entity::user::ActiveModel = user.into();
            active.password_hash = Set(hash);
            active.update(&db).await.unwrap();

            println!("Password changed for '{username}'.");
        }
        Some("storage") if args.get(2).map(String::as_str) == Some("migrate") => {
            match storage_migrate(&args[3..]).await {
                Ok(true) => {}
                Ok(false) => std::process::exit(1),
                Err(e) => {
                    eprintln!("storage migrate: {e:#}");
                    std::process::exit(1);
                }
            }
        }
        Some("design") if args.get(2).map(String::as_str) == Some("push") => {
            match design_push(&args[3..]).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("design push: {e:#}");
                    std::process::exit(1);
                }
            }
        }
        Some("design") if args.get(2).map(String::as_str) == Some("contract") => {
            print!("{}", site::templates::contract::markdown());
        }
        _ => {
            eprintln!("Usage: site_cli <command>");
            eprintln!("Commands:");
            eprintln!("  create-user <username> <password>       Create a user");
            eprintln!("  change-password <username> <password>   Change password");
            eprintln!(
                "  storage migrate --from db | --from-dir <path>\n                                          Copy every blob and object into the configured STORAGE_KIND"
            );
            eprintln!(
                "  design push <dir>                       Upload a design folder into the design draft\n                                          (not while an admin publishes, discards or restores)"
            );
            eprintln!(
                "  design contract                         Print the template contract (docs/design-contract.md)"
            );
            std::process::exit(1);
        }
    }
}

/// Copy every blob and keyed object into the configured backend; `Ok(false)`
/// when any could not be copied (the report lists which).
async fn storage_migrate(args: &[String]) -> anyhow::Result<bool> {
    use anyhow::{Context as _, bail};
    use site::storage::{Storage, StorageConfig, migrate};

    let db = Database::connect(std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?)
        .await
        .context("connect to the database")?;
    let source = match args {
        [flag, kind] if flag == "--from" && kind == "db" => Storage::db(db.clone()),
        [flag, dir] if flag == "--from-dir" => {
            Storage::local(std::path::Path::new(dir), db.clone())?
        }
        _ => bail!("usage: site_cli storage migrate --from db | --from-dir <path>"),
    };
    let target = Storage::new(&StorageConfig::from_env()?, db)?;
    println!("{} → {}", source.kind(), target.kind());

    let hashes = target.known_hashes().await?;
    let report = migrate::migrate(&source, &target, &hashes).await?;
    let mut ok = true;
    for problem in report.problems() {
        eprintln!("  {problem}");
        ok = false;
    }
    println!("{}", report.summary());
    Ok(ok)
}

/// Upload a design folder into the shared draft; it goes live when an admin
/// publishes the draft.
async fn design_push(args: &[String]) -> anyhow::Result<()> {
    use anyhow::{Context as _, bail};
    use site::storage::{Storage, StorageConfig};

    let [dir] = args else {
        bail!("usage: site_cli design push <dir>");
    };
    let db = Database::connect(std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?)
        .await
        .context("connect to the database")?;
    let storage = Storage::new(&StorageConfig::from_env()?, db)?;
    let report = site::design::push::push(&storage, std::path::Path::new(dir)).await?;
    for skipped in &report.skipped {
        println!("  skipped (outside templates/, assets/, mdcast/): {skipped}");
    }
    println!(
        "{} uploaded into the draft, {} unchanged, {} skipped — review and publish it in the admin Design page (refresh open tabs: a push sends no live update)",
        report.uploaded.len(),
        report.unchanged.len(),
        report.skipped.len()
    );
    Ok(())
}
