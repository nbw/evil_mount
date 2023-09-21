use anyhow::{anyhow, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::{self, remove_dir_all, remove_file},
    sync::RwLock,
};
use walkdir::WalkDir;

use clap::Parser;

/// A program to backup files to a different directory
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The directory that you will be working in, will be completely cleared
    #[arg(short, long)]
    work_dir: PathBuf,

    /// The directory that will be copied to. Used to initialize source dir
    #[arg(short, long)]
    backup_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args {
        work_dir,
        backup_dir,
    } = Args::parse();

    // Ensure that source_dir and backup_dir are folders
    if !work_dir.is_dir() {
        return Err(anyhow!("work_dir must be a directory!"));
    }

    if !backup_dir.is_dir() {
        return Err(anyhow!("backup_dir must be a directory!"));
    }

    println!("Clearing {}...", work_dir.display());
    while let Ok(Some(file_info)) = fs::read_dir(&work_dir)
        .await
        .map_err(|err| anyhow!("Error reading the source directory: {err}"))?
        .next_entry()
        .await
    {
        let path = file_info.path();
        match path.is_dir() {
            true => remove_dir_all(&path).await?,
            false => match path.is_file() {
                true => remove_file(&path).await?,
                // not really sure what to do here
                false => todo!(),
            },
        };
    }
    println!("Cleared {}!", work_dir.display());

    // TODO: don't initialize if work-dir is identical to other dir
    println!(
        "Initializing {} with the contents of {}...",
        work_dir.display(),
        backup_dir.display()
    );
    for file_info in WalkDir::new(&backup_dir)
        .follow_links(true)
        .into_iter()
        .filter(|file_info| match file_info {
            Ok(file_info) => file_info.path().is_file(),
            Err(_) => false,
        })
        .into_iter()
    {
        let file_info = file_info?;
        let path = file_info.path();
        copy_to_dst(path.to_path_buf(), backup_dir.clone(), work_dir.clone())
            .await
            .map_err(|err| anyhow!("Error copying file for initialization: {err}"))?;
    }

    println!("Initialized {}!", work_dir.display());

    tokio::task::spawn(async move { copy_files(work_dir, backup_dir).await.unwrap() });

    tokio::signal::ctrl_c().await?;

    println!("Done!");

    Ok(())
}

async fn backup_files() {
    todo!()
}

async fn copy_files(work_dir: PathBuf, backup_dir: PathBuf) -> Result<()> {
    let modify_times: Arc<RwLock<HashMap<PathBuf, u64>>> = Arc::new(RwLock::new(HashMap::new()));

    loop {
        // Get the modification times of all the files we're tracking
        let new_modify_times = {
            let mut modify_times: HashMap<PathBuf, u64> = HashMap::new();

            // TODO: just use map
            for file_info in WalkDir::new(&work_dir)
                .follow_links(true)
                .into_iter()
                .filter(|file_info| match file_info {
                    Ok(file_info) => file_info.path().is_file(),
                    Err(_) => false,
                })
            {
                let file_info = file_info?;

                let metadata = fs::metadata(file_info.path()).await.unwrap();
                let modify_time = metadata
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                modify_times.insert(file_info.path().to_path_buf(), modify_time);
            }

            modify_times
        };

        let handles = new_modify_times.into_iter().map(|(path, new_mod_time)| {
            let modify_times = modify_times.clone();
            let work_dir = work_dir.clone();
            let backup_dir = backup_dir.clone();

            async move {
                let old_mod_time = {
                    let modify_times_lock = modify_times.read().await;
                    modify_times_lock.get(&path).cloned()
                };

                if let Some(old_mod_time) = old_mod_time {
                    // The file was modified, so copy it
                    if new_mod_time > old_mod_time {
                        return copy_to_dst(path, work_dir, backup_dir).await;
                    }
                } else {
                    // The file was just added, so just copy it
                    {
                        let modify_times = &mut modify_times.write().await;
                        modify_times.insert(
                            path.clone(),
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_secs(),
                        );
                    }
                    return copy_to_dst(path, work_dir, backup_dir).await;
                }

                Ok(())
            }
        });

        futures::future::join_all(handles)
            .await
            .iter()
            .for_each(|res| {
                if let Err(err) = res {
                    eprintln!("Error syncing file: {err:?}");
                }
            });

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn copy_to_dst(path: PathBuf, work_dir: PathBuf, backup_dir: PathBuf) -> Result<()> {
    let new_path = path.strip_prefix(&work_dir).map_err(|err| {
        anyhow!(
            "Error stripping prefix {} from {}: {err}",
            work_dir.display(),
            path.display()
        )
    })?;
    let mut dst_path = backup_dir.clone();
    dst_path.push(new_path);

    let backup_dir = {
        let mut dst_path = dst_path.clone();
        dst_path.pop();
        dst_path
    };

    fs::create_dir_all(&backup_dir).await?;
    fs::copy(path.clone(), dst_path.clone())
        .await
        .map_err(|err| {
            anyhow!(
                "Error copying from {} to {}: {err}",
                path.display(),
                dst_path.display()
            )
        })?;

    Ok(())
}
