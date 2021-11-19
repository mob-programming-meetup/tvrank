#![warn(clippy::all)]

use super::{
  basics::Basics,
  title::{Title, TitleId},
};
use crate::{
  imdb::{ratings::Ratings, storage::Storage},
  Res,
};
use crossbeam::thread;
use log::{debug, error, info};
use parking_lot::const_mutex;
use std::{ops::DerefMut, path::Path, sync::Arc};

pub struct Service {
  basics_dbs: Vec<Basics>,
  ratings_db: Ratings,
}

impl Service {
  fn parse_basics(n: usize, storage: &Storage) -> Res<Vec<Basics>> {
    let mut basics_dbs = Vec::with_capacity(n);

    let basics_source = storage.basics_db_buf.split(|&b| b == b'\n').skip(1);
    let basics_source = Arc::new(const_mutex(basics_source));

    let _ = thread::scope(|s| {
      let mut handles = Vec::with_capacity(n);

      for i in 0..n {
        let basics_source = basics_source.clone();

        let handle =
          s.builder().name(format!("IMDB Parsing Thread #{}", i)).spawn(move |_| {
            let mut basics_db = Basics::default();
            let mut lines = Vec::new();

            loop {
              lines.extend(basics_source.lock().deref_mut().take(200_000));

              if lines.is_empty() {
                break;
              }

              for &line in &lines {
                if !line.is_empty() {
                  if let Err(e) = basics_db.add_basics_from_line(line) {
                    panic!("On DB Thread {}: Cannot parse DB: {}", i, e)
                  }
                }
              }

              lines.clear();
            }

            basics_db
          });

        match handle {
          Ok(handle) => handles.push(handle),
          Err(e) => error!("Could not spawn thread {} for parsing DB: {}", i, e),
        }
      }

      for (i, handle) in handles.into_iter().enumerate() {
        match handle.join() {
          Ok(basics_db) => basics_dbs.push(basics_db),
          Err(e) => error!("Could not join thread {}: {:?}", i, e),
        }
      }
    });

    Ok(basics_dbs)
  }

  pub fn new(app_cache_dir: &Path) -> Res<Self> {
    let ncpus = num_cpus::get() / 2;
    debug!("Going to use {} threads", ncpus);

    info!("Loading IMDB Databases...");
    let storage = Storage::load_db_files(app_cache_dir)?;

    info!("Parsing IMDB Basics DB...");
    let basics_dbs = Self::parse_basics(ncpus, &storage)?;
    info!("Done parsing IMDB Basics DB");

    info!("Parsing IMDB Ratings DB...");
    let ratings_db = Ratings::new_from_buf(&storage.ratings_db_buf)?;
    info!("Done parsing IMDB Ratings DB");

    let mut total_movies = 0;
    let mut total_series = 0;
    for (i, db) in basics_dbs.iter().enumerate() {
      let n_movies = db.n_movies();
      let n_series = db.n_series();
      total_movies += n_movies;
      total_series += n_series;
      debug!("DB {} has {} movies and {} series", i, n_movies, n_series);
    }
    debug!("DB has a total of {} movies and {} series", total_movies, total_series);

    Ok(Self { basics_dbs, ratings_db })
  }

  fn query(&self, f: impl Fn(&Basics) -> Vec<&Title> + Copy + Send) -> Res<Vec<&Title>> {
    let mut res = Vec::new();

    let _ = thread::scope(|s| {
      let mut handles = Vec::with_capacity(self.basics_dbs.len());
      let mut dbs: &[Basics] = self.basics_dbs.as_slice();

      for i in 0..self.basics_dbs.len() {
        let (db, rem) = match dbs.split_first() {
          Some(res) => res,
          None => break,
        };

        dbs = rem;

        let thread_name = format!("IMDB Query Thread #{}", i);
        let handle = s.builder().name(thread_name).spawn(move |_| f(db));

        match handle {
          Ok(handle) => handles.push(handle),
          Err(e) => error!("Could not spawn thread {} for movie querying: {}", i, e),
        }
      }

      for (i, handle) in handles.into_iter().enumerate() {
        match handle.join() {
          Ok(titles) => res.extend_from_slice(titles.as_slice()),
          Err(e) => error!("Could not join thread {}: {:?}", i, e),
        }
      }
    });

    Ok(res)
  }

  pub fn movie(&self, name: &str, year: Option<u16>) -> Res<Vec<&Title>> {
    self.query(|db| {
      if let Some(year) = year {
        db.movie_with_year(name, year)
      } else {
        db.movie(name)
      }
    })
  }

  pub fn series(&self, name: &str, year: Option<u16>) -> Res<Vec<&Title>> {
    self.query(|db| {
      if let Some(year) = year {
        db.series_with_year(name, year)
      } else {
        db.series(name)
      }
    })
  }

  pub fn rating(&self, title_id: TitleId) -> Option<&(u8, u64)> {
    self.ratings_db.get(title_id)
  }
}
