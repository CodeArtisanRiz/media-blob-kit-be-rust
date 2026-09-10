mod entities;
mod routes;
mod middleware;
pub mod config;
mod error;
mod pagination;
pub mod services;
pub mod models;
pub mod utils;



use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use clap::{Parser, Subcommand};
use entities::user;
use migration::{Migrator, MigratorTrait};
use routes::create_routes;
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Apply pending migrations
    Migrate,
    /// Reset database (refresh migrations)
    Reset,
    /// Create a superuser
    CreateSuperuser {
        #[arg(short, long)]
        username: String,
    },
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    
    // Initialize structured tracing logger
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Initialize config
    let config = config::get_config();
    
    let db = Database::connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Migrate) => {
            Migrator::up(&db, None).await.expect("Migration failed");
            println!("Migrations applied successfully");
        }
        Some(Commands::Reset) => {
            Migrator::refresh(&db).await.expect("Migration refresh failed");
            println!("Database reset successfully");
        }
        Some(Commands::CreateSuperuser { username }) => {
            let password = rpassword::prompt_password("Enter password: ").unwrap();
            let salt = SaltString::generate(&mut OsRng);
            let argon2 = Argon2::default();
            let password_hash = argon2
                .hash_password(password.as_bytes(), &salt)
                .unwrap()
                .to_string();

            let user = user::ActiveModel {
                id: Set(Uuid::new_v4()),
                username: Set(username.clone()),
                password: Set(password_hash),
                role: Set(user::Role::Su),
                created_at: Set(chrono::Utc::now().naive_utc()),
                ..Default::default()
            };

            match user.insert(&db).await {
                Ok(_) => println!("Superuser '{}' created successfully", username),
                Err(e) => eprintln!("Failed to create superuser: {}", e),
            }
        }
        None => {
            // build our application using the routes module
            let app = create_routes(db.clone())
                .layer(tower_http::cors::CorsLayer::permissive());

            // Auto-create superuser if configured
            if let (Some(username), Some(password)) = (&config.su_username, &config.su_password) {
                let user_exists = user::Entity::find()
                    .filter(user::Column::Username.eq(username))
                    .one(&db)
                    .await
                    .expect("Failed to check for existing user");

                if user_exists.is_none() {
                    let salt = SaltString::generate(&mut OsRng);
                    let argon2 = Argon2::default();
                    let password_hash = argon2
                        .hash_password(password.as_bytes(), &salt)
                        .unwrap()
                        .to_string();

                    let user = user::ActiveModel {
                        id: Set(Uuid::new_v4()),
                        username: Set(username.clone()),
                        password: Set(password_hash),
                        role: Set(user::Role::Su),
                        created_at: Set(chrono::Utc::now().naive_utc()),
                        ..Default::default()
                    };

                    match user.insert(&db).await {
                        Ok(_) => println!("Auto-created superuser '{}'", username),
                        Err(e) => eprintln!("Failed to auto-create superuser: {}", e),
                    }
                } else {
                    println!("Superuser '{}' already exists, skipping creation", username);
                }
            }

            // Spawn background worker
            let worker_db = db.clone();
            tokio::spawn(async move {
                let worker = services::worker::Worker::new(worker_db).await;
                worker.run().await;
            });

            // Spawn cleanup scheduler
            let cleanup_db = db.clone();
            tokio::spawn(async move {
                let cleanup = services::cleanup::CleanupService::new(cleanup_db);
                cleanup.run_scheduler().await;
            });

            // run our app listening on configured host and port
            let addr = format!("{}:{}", config.host, config.port);
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .unwrap_or_else(|e| panic!("Failed to bind to {}: {}", addr, e));
            println!("Listening on {}", listener.local_addr().unwrap());
            axum::serve(listener, app).await.unwrap();
        }
    }
}
