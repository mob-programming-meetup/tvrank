#![warn(clippy::all)]

mod ui;

use atoi::atoi;
use derive_more::Display;
use directories::ProjectDirs;
use humantime::format_duration;
use indicatif::{HumanBytes, ProgressBar};
use log::{debug, error, info, trace, warn};
use prettytable::{color, format, Attr, Cell, Row, Table};
use regex::Regex;
use reqwest::Url;
use serde::Deserialize;
use std::cmp::Ordering;
use std::error::Error;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;
use structopt::StructOpt;
use tvrank::imdb::{Imdb, ImdbKeywordSet, ImdbQueryType, ImdbStorage, ImdbTitle, ImdbTitleId};
use tvrank::Res;
use ui::{create_progress_bar, create_progress_spinner};
use walkdir::WalkDir;

#[derive(Debug, Display)]
#[display(fmt = "{}")]
enum TvRankErr {
  #[display(fmt = "Could not find cache directory")]
  CacheDir,
  #[display(fmt = "Short, invalid or empty keywords")]
  BadKeywords,
}

impl TvRankErr {
  fn cache_dir<T>() -> Res<T> {
    Err(Box::new(TvRankErr::CacheDir))
  }
}

impl Error for TvRankErr {}

fn parse_name_and_year(input: &str) -> Option<(&str, u16)> {
  let regex = match Regex::new(r"^(.+)\s+\((\d{4})\)$") {
    Ok(regex) => regex,
    Err(e) => {
      warn!("Could not parse input `{}` as TITLE (YYYY): {}", input, e);
      return None;
    }
  };

  if let Some(captures) = regex.captures(input) {
    if let Some(title_match) = captures.get(1) {
      if let Some(year_match) = captures.get(2) {
        if let Some(year_val) = atoi::<u16>(year_match.as_str().as_bytes()) {
          let title = title_match.as_str();
          Some((title, year_val))
        } else {
          warn!("Could not parse year `{}`", year_match.as_str());
          None
        }
      } else {
        warn!("Could not parse year from `{}`", input);
        None
      }
    } else {
      warn!("Could not parse title from `{}`", input);
      None
    }
  } else {
    warn!("Could not parse title and year from `{}`", input);
    None
  }
}

fn create_project() -> Res<ProjectDirs> {
  let prj = ProjectDirs::from("com.fredmorcos", "Fred Morcos", "tvrank");
  if let Some(prj) = prj {
    Ok(prj)
  } else {
    TvRankErr::cache_dir()
  }
}

#[derive(Debug, StructOpt)]
#[structopt(about = "Query information about movies and series")]
#[structopt(author = "Fred Morcos <fm@fredmorcos.com>")]
struct Opt {
  /// Verbose output (can be specified multiple times)
  #[structopt(short, long, parse(from_occurrences))]
  verbose: u8,

  /// Force updating internal databases.
  #[structopt(short, long)]
  force_update: bool,

  /// Sort by year/rating/title instead of rating/year/title
  #[structopt(short = "y", long)]
  sort_by_year: bool,

  #[structopt(subcommand)]
  command: Command,
}

#[derive(Debug, StructOpt)]
enum Command {
  /// Lookup a single title using "TITLE" or "TITLE (YYYY)"
  Title {
    #[structopt(name = "TITLE")]
    title: String,
  },
  /// Lookup movie titles from a directory
  MoviesDir {
    #[structopt(name = "DIR")]
    dir: PathBuf,
  },
  /// Lookup series titles from a directory
  SeriesDir {
    #[structopt(name = "DIR")]
    dir: PathBuf,
  },
}

fn sort_results(results: &mut Vec<ImdbTitle>, sort_by_year: bool) {
  if sort_by_year {
    results.sort_unstable_by(|a, b| {
      match b.start_year().cmp(&a.start_year()) {
        Ordering::Equal => {}
        ord => return ord,
      }
      match b.rating().cmp(&a.rating()) {
        Ordering::Equal => {}
        ord => return ord,
      }
      b.primary_title().cmp(a.primary_title())
    })
  } else {
    results.sort_unstable_by(|a, b| {
      match b.rating().cmp(&a.rating()) {
        Ordering::Equal => {}
        ord => return ord,
      }
      match b.start_year().cmp(&a.start_year()) {
        Ordering::Equal => {}
        ord => return ord,
      }
      b.primary_title().cmp(a.primary_title())
    })
  }
}

fn create_output_table() -> Table {
  let mut table = Table::new();

  let table_format = format::FormatBuilder::new()
    .column_separator('│')
    .borders('│')
    .padding(1, 1)
    .build();

  table.set_format(table_format);

  table.add_row(Row::new(vec![
    Cell::new("Primary Title").with_style(Attr::Bold),
    Cell::new("Original Title").with_style(Attr::Bold),
    Cell::new("Year").with_style(Attr::Bold),
    Cell::new("Rating").with_style(Attr::Bold),
    Cell::new("Votes").with_style(Attr::Bold),
    Cell::new("Runtime").with_style(Attr::Bold),
    Cell::new("Genres").with_style(Attr::Bold),
    Cell::new("Type").with_style(Attr::Bold),
    Cell::new("IMDB ID").with_style(Attr::Bold),
    Cell::new("IMDB Link").with_style(Attr::Bold),
  ]));

  table
}

fn create_output_table_row_for_title(title: &ImdbTitle, imdb_url: &Url) -> Res<Row> {
  static GREEN: Attr = Attr::ForegroundColor(color::GREEN);
  static YELLOW: Attr = Attr::ForegroundColor(color::YELLOW);
  static RED: Attr = Attr::ForegroundColor(color::RED);

  let mut row = Row::new(vec![]);

  row.add_cell(Cell::new(title.primary_title()));

  if title.primary_title() == title.original_title() {
    row.add_cell(Cell::new(""));
  } else {
    row.add_cell(Cell::new(title.original_title()));
  }

  if let Some(year) = title.start_year() {
    row.add_cell(Cell::new(&format!("{}", year)));
  } else {
    row.add_cell(Cell::new(""));
  }

  if let Some(&(rating, votes)) = title.rating() {
    let rating_text = &format!("{}/100", rating);

    let rating_cell = Cell::new(rating_text).with_style(match rating {
      rating if rating >= 70 => GREEN,
      rating if (60..70).contains(&rating) => YELLOW,
      _ => RED,
    });

    row.add_cell(rating_cell);
    row.add_cell(Cell::new(&format!("{}", votes)));
  } else {
    row.add_cell(Cell::new(""));
    row.add_cell(Cell::new(""));
  }

  if let Some(runtime) = title.runtime() {
    row.add_cell(Cell::new(&format_duration(runtime).to_string()));
  } else {
    row.add_cell(Cell::new(""));
  }

  row.add_cell(Cell::new(&format!("{}", title.genres())));
  row.add_cell(Cell::new(&format!("{}", title.title_type())));

  let title_id = title.title_id();
  row.add_cell(Cell::new(&format!("{}", title_id)));

  let url = imdb_url.join(&format!("{}", title_id))?;
  row.add_cell(Cell::new(url.as_str()));

  Ok(row)
}

fn setup_imdb_storage(app_cache_dir: &Path, force_update: bool) -> Res<ImdbStorage> {
  info!("Loading IMDB Databases...");

  // Downloading callbacks.
  let download_init = |name: &str, content_len: Option<u64>| -> ProgressBar {
    let msg = format!("Downloading {}", name);
    let bar = if let Some(file_len) = content_len {
      info!("{} compressed file size is {}", name, HumanBytes(file_len));
      create_progress_bar(msg, file_len)
    } else {
      info!("{} compressed file size is unknown", name);
      create_progress_spinner(msg)
    };

    bar
  };

  let download_progress = |bar: &ProgressBar, delta: u64| {
    bar.inc(delta);
  };

  let download_finish = |bar: &ProgressBar| {
    bar.finish_and_clear();
  };

  // Extraction callbacks.
  let extract_init = |name: &str| -> ProgressBar {
    let msg = format!("Decompressing {}...", name);
    create_progress_spinner(msg)
  };

  let extract_progress = |bar: &ProgressBar, delta: u64| {
    bar.inc(delta);
  };

  let extract_finish = |bar: &ProgressBar| {
    bar.finish_and_clear();
  };

  let imdb_storage = ImdbStorage::new(
    app_cache_dir,
    force_update,
    &(download_init, download_progress, download_finish),
    &(extract_init, extract_progress, extract_finish),
  )?;

  Ok(imdb_storage)
}

fn imdb_lookup_by_title_year<'a>(
  name: &str,
  year: Option<u16>,
  imdb: &'a Imdb,
  query_type: ImdbQueryType,
  results: &mut Vec<ImdbTitle<'a, 'a>>,
) -> Res<()> {
  results.extend(imdb.by_title(query_type, &name.to_lowercase(), year)?);
  Ok(())
}

fn imdb_lookup_by_keywords<'a>(
  keywords: ImdbKeywordSet,
  imdb: &'a Imdb,
  query_type: ImdbQueryType,
  results: &mut Vec<ImdbTitle<'a, 'a>>,
) -> Res<()> {
  results.extend(imdb.by_keywords(query_type, keywords)?);
  Ok(())
}

fn imdb_lookup_by_titleid<'a>(
  title_id: &ImdbTitleId,
  imdb: &'a Imdb,
  query_type: ImdbQueryType,
  results: &mut Vec<ImdbTitle<'a, 'a>>,
) -> Res<()> {
  results.extend(imdb.by_titleid(query_type, title_id)?);
  Ok(())
}

fn display_title(name: &str, year: Option<u16>) -> String {
  format!(
    "{}{}",
    name,
    if let Some(year) = year {
      format!(" ({})", year)
    } else {
      "".to_string()
    }
  )
}

fn single_title<'a>(title: &str, imdb: &'a Imdb, imdb_url: &Url, sort_by_year: bool) -> Res<()> {
  let mut keywords = None;

  let (name, year) = if let Some((name, year)) = parse_name_and_year(title) {
    (name, Some(year))
  } else {
    warn!("Going to use `{}` as keywords for search query", title);
    let keywords_map = ImdbKeywordSet::try_from(title).map_err(|_| TvRankErr::BadKeywords)?;
    info!("Keywords: {}", keywords_map);
    keywords = Some(keywords_map);
    (title, None)
  };

  let mut movies_results = vec![];

  if let Some(keywords) = &keywords {
    imdb_lookup_by_keywords(keywords.clone(), imdb, ImdbQueryType::Movies, &mut movies_results)?;
  } else {
    imdb_lookup_by_title_year(name, year, imdb, ImdbQueryType::Movies, &mut movies_results)?;
  }

  if movies_results.is_empty() {
    eprintln!("No movie matches found for `{}`", display_title(name, year));
  } else {
    eprintln!(
      "Found {} movie {} for `{}`:",
      movies_results.len(),
      if movies_results.len() == 1 {
        "match"
      } else {
        "matches"
      },
      display_title(name, year)
    );

    sort_results(&mut movies_results, sort_by_year);

    let mut table = create_output_table();

    for res in &movies_results {
      let row = create_output_table_row_for_title(res, imdb_url)?;
      table.add_row(row);
    }

    table.printstd();
  }

  let mut series_results = vec![];
  if let Some(keywords) = &keywords {
    imdb_lookup_by_keywords(keywords.clone(), imdb, ImdbQueryType::Series, &mut series_results)?;
  } else {
    imdb_lookup_by_title_year(name, year, imdb, ImdbQueryType::Series, &mut series_results)?;
  }

  if series_results.is_empty() {
    eprintln!("No series matches found for `{}`", display_title(name, year));
  } else {
    eprintln!(
      "Found {} series {} for `{}`:",
      series_results.len(),
      if series_results.len() == 1 {
        "match"
      } else {
        "matches"
      },
      display_title(name, year)
    );

    sort_results(&mut series_results, sort_by_year);

    let mut table = create_output_table();

    for res in &series_results {
      let row = create_output_table_row_for_title(res, imdb_url)?;
      table.add_row(row);
    }

    table.printstd();
  }

  Ok(())
}

#[derive(Deserialize)]
struct TitleInfo {
  imdb: ImdbTitleInfo,
}

#[derive(Deserialize)]
struct ImdbTitleInfo {
  id: String,
}

fn titles_dir<'a>(
  dir: &Path,
  imdb: &'a Imdb,
  query_type: ImdbQueryType,
  imdb_url: &Url,
  series: bool,
  sort_by_year: bool,
) -> Res<()> {
  let mut at_least_one = false;
  let mut at_least_one_matched = false;
  let mut results = vec![];

  let walkdir = WalkDir::new(dir).min_depth(1);
  let walkdir = if series {
    walkdir.max_depth(1)
  } else {
    walkdir
  };

  for entry in walkdir {
    let entry = entry?;

    if entry.file_type().is_dir() {
      let entry_path = entry.path();

      let title_info_path = entry_path.join("tvrank.json");
      if title_info_path.exists() {
        let title_info_file = fs::File::open(&title_info_path)?;
        let title_info_file_reader = BufReader::new(title_info_file);
        let title_info: Result<TitleInfo, _> = serde_json::from_reader(title_info_file_reader);

        match title_info {
          Ok(info) => match ImdbTitleId::try_from(info.imdb.id.as_ref()) {
            Ok(title_id) => {
              let mut local_results = vec![];
              imdb_lookup_by_titleid(&title_id, imdb, query_type, &mut local_results)?;

              if local_results.is_empty() {
                warn!(
                  "Could not find title ID `{}` for `{}`, ignoring `tvrank.json` file",
                  title_id,
                  title_info_path.display()
                );
              } else if local_results.len() > 1 {
                warn!("Found {} matches for title ID `{}` for `{}`, this should not happen, ignoring `tvrank.json` file",
                      local_results.len(), title_id, title_info_path.display());
              } else {
                at_least_one_matched = true;
                results.extend(local_results);
                continue;
              }
            }
            Err(e) => warn!("Ignoring IMDB ID in `{}` due to parse error: {}", title_info_path.display(), e),
          },
          Err(e) => warn!("Ignoring info in `{}` due to parse error: {}", title_info_path.display(), e),
        }
      }

      if let Some(filename) = entry_path.file_name() {
        let filename = filename.to_string_lossy();

        let (name, year) = if let Some((name, year)) = parse_name_and_year(&filename) {
          at_least_one = true;
          (name, Some(year))
        } else if series {
          (filename.as_ref(), None)
        } else {
          warn!(
            "Skipping `{}` because `{}` does not follow the TITLE (YYYY) format",
            entry.path().display(),
            filename,
          );

          continue;
        };

        let mut local_results = vec![];
        imdb_lookup_by_title_year(name, year, imdb, query_type, &mut local_results)?;

        if local_results.is_empty() {
          eprintln!("No matches found for `{}`", display_title(name, year));
        } else if local_results.len() > 1 {
          at_least_one_matched = true;

          eprintln!("Found {} matche(s) for `{}`:", local_results.len(), display_title(name, year));

          sort_results(&mut local_results, sort_by_year);

          let mut table = create_output_table();

          for res in &local_results {
            let row = create_output_table_row_for_title(res, imdb_url)?;
            table.add_row(row);
          }

          table.printstd();
        } else {
          at_least_one_matched = true;
          results.extend(local_results);
        }
      }
    }
  }

  if !at_least_one {
    println!("No valid directory names");
    return Ok(());
  }

  if !at_least_one_matched {
    println!("None of the directories matched any titles");
    return Ok(());
  }

  sort_results(&mut results, sort_by_year);

  let mut table = create_output_table();

  for res in &results {
    let row = create_output_table_row_for_title(res, imdb_url)?;
    table.add_row(row);
  }

  table.printstd();

  Ok(())
}

fn run(opt: &Opt) -> Res<()> {
  let project = create_project()?;
  let app_cache_dir = project.cache_dir();
  info!("Cache directory: {}", app_cache_dir.display());

  fs::create_dir_all(app_cache_dir)?;
  debug!("Created cache directory");

  const IMDB: &str = "https://www.imdb.com/title/";
  let imdb_url = Url::parse(IMDB)?;

  let start_time = Instant::now();
  let imdb_storage = setup_imdb_storage(app_cache_dir, opt.force_update)?;

  let ncpus = rayon::current_num_threads();
  let imdb = Imdb::new(ncpus / 2, &imdb_storage)?;
  eprintln!("Loaded IMDB database in {}", format_duration(Instant::now().duration_since(start_time)));

  let start_time = Instant::now();

  match &opt.command {
    Command::Title { title } => single_title(title, &imdb, &imdb_url, opt.sort_by_year)?,
    Command::MoviesDir { dir } => {
      titles_dir(dir, &imdb, ImdbQueryType::Movies, &imdb_url, false, opt.sort_by_year)?
    }
    Command::SeriesDir { dir } => {
      titles_dir(dir, &imdb, ImdbQueryType::Series, &imdb_url, true, opt.sort_by_year)?
    }
  }

  eprintln!("IMDB query took {}", format_duration(Instant::now().duration_since(start_time)));

  std::mem::forget(imdb);

  Ok(())
}

fn main() {
  let start_time = Instant::now();
  let opt = Opt::from_args();

  let log_level = match opt.verbose {
    0 => log::LevelFilter::Off,
    1 => log::LevelFilter::Error,
    2 => log::LevelFilter::Warn,
    3 => log::LevelFilter::Info,
    4 => log::LevelFilter::Debug,
    _ => log::LevelFilter::Trace,
  };

  let logger = env_logger::Builder::new().filter_level(log_level).try_init();
  let have_logger = if let Err(e) = logger {
    eprintln!("Error initializing logger: {}", e);
    false
  } else {
    true
  };

  error!("Error output enabled.");
  warn!("Warning output enabled.");
  info!("Info output enabled.");
  debug!("Debug output enabled.");
  trace!("Trace output enabled.");

  if let Err(e) = run(&opt) {
    if have_logger {
      error!("Error: {}", e);
    } else {
      eprintln!("Error: {}", e);
    }
  }

  eprintln!("Total time: {}", format_duration(Instant::now().duration_since(start_time)));
}
