#![warn(clippy::all)]

mod parsing;

pub mod basics;
pub mod error;
pub mod genre;
pub mod ratings;
pub mod service;
pub mod storage;
pub mod title;

pub use error::Err as ImdbErr;
pub use genre::{Genre as ImdbGenre, Genres as ImdbGenres};
pub use service::Service as Imdb;
pub use title::{Title as ImdbTitle, TitleId as ImdbTitleId, TitleType as ImdbTitleType};
