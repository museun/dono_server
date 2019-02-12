use log::*;
use serde::{Deserialize, Serialize};

mod local;
mod youtube;

mod database;
mod error;
mod server;

use server::HttpServer;

use error::Error;
type Result<T> = std::result::Result<T, Error>;

fn main() {
    env_logger::Builder::from_default_env()
        .default_format_timestamp(false)
        .init();

    let dir = directories::ProjectDirs::from("com.github", "museun", "dono_server").unwrap();
    std::fs::create_dir_all(dir.data_dir()).expect("must be able to create project dirs");
    std::fs::create_dir_all(dir.config_dir()).expect("must be able to create project dirs");

    #[derive(Deserialize, Serialize, Clone, Debug)]
    struct Config {
        pub address: String,
        pub port: u16,
    }

    let file = dir.config_dir().join("config.toml");
    let config: Config = match std::fs::read(&file)
        .ok()
        .and_then(|data| toml::from_slice(&data).ok())
    {
        Some(config) => config,
        None => {
            warn!("creating default config.toml at {}", file.to_str().unwrap());
            warn!("edit and re-run");
            let data = toml::to_string_pretty(&Config {
                address: "localhost".into(),
                port: 50006,
            })
            .expect("valid config");
            std::fs::write(file, &data).expect("write config");
            std::process::exit(1)
        }
    };

    database::DB_PATH
        .set(dir.data_dir().join("videos.db"))
        .expect("must be able to set DB path");

    if let Err(err) = database::get_connection()
        .execute_batch(include_str!("../sql/schema.sql"))
        .map_err(Error::Sql)
    {
        error!("cannot create tables from schema: {}", err);
        std::process::exit(1)
    }

    let server = match HttpServer::new((config.address.as_str(), config.port)) {
        Ok(server) => server,
        Err(err) => {
            error!("cannot start http server: {}", err);
            std::process::exit(1)
        }
    };

    server.run()
}

pub trait Storage<T>
where
    T: FromRow,
{
    fn insert(&self, item: &server::Item) -> Result<()>;
    fn current(&self) -> Result<T>;
    fn previous(&self) -> Result<T>;
    fn all(&self) -> Result<Vec<T>>;
}

pub trait FromRow {
    fn from_row(row: &rusqlite::Row<'_, '_>) -> Self;
    fn timestamp(&self) -> i64;
}
