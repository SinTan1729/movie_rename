use inquire::{
    ui::{Color, IndexPrefix, RenderConfig, Styled},
    InquireError, Select,
};
use std::{collections::HashMap, fs, path::Path, process::exit};
use tmdb_api::{
    movie::{credits::MovieCredits, search::MovieSearch},
    prelude::Command,
    Client,
};
use torrent_name_parser::Metadata;

use crate::structs::{get_long_lang, Language, MovieEntry};

// Get the version from Cargo.toml
const VERSION: &str = env!("CARGO_PKG_VERSION");

// Function to process movie entries
pub async fn process_file(
    filename: &String,
    tmdb: &Client,
    pattern: &str,
    dry_run: bool,
    lucky: bool,
    movie_list: Option<&HashMap<String, Option<String>>>,
    // The last bool tells whether the entry should be added to the movie_list or not
    // The first String is filename without extension, and the second String is
    // new basename, if any.
) -> (String, Option<String>, bool) {
    // Set RenderConfig for the menu items
    inquire::set_global_render_config(get_render_config());

    // Get the basename
    let mut file_base = String::from(filename);
    let mut parent = String::new();
    if let Some(parts) = filename.rsplit_once('/') {
        {
            parent = String::from(parts.0);
            file_base = String::from(parts.1);
        }
    }

    // Split the filename into parts for a couple of checks and some later use
    let filename_parts: Vec<&str> = filename.rsplit('.').collect();
    let filename_without_ext = if filename_parts.len() >= 3 && filename_parts[1].len() == 2 {
        filename.rsplitn(3, '.').last().unwrap().to_string()
    } else {
        filename.rsplit_once('.').unwrap().0.to_string()
    };

    // Check if the filename (without extension) has already been processed
    // If yes, we'll use the older results
    let mut preprocessed = false;
    let mut new_name_base = match movie_list {
        None => String::new(),
        Some(list) => {
            if list.contains_key(&filename_without_ext) {
                preprocessed = true;
                list[&filename_without_ext].clone().unwrap_or_default()
            } else {
                String::new()
            }
        }
    };

    // Check if it should be ignored
    if preprocessed && new_name_base.is_empty() {
        eprintln!("  Ignoring {file_base} as per previous choice for related files...");
        return (filename_without_ext, None, false);
    }

    // Parse the filename for metadata
    let metadata = Metadata::from(file_base.as_str()).expect("  Could not parse filename!");

    // Process only if it's a valid file format
    let mut extension = metadata.extension().unwrap_or("").to_string();
    if ["mp4", "avi", "mkv", "flv", "m4a", "srt", "ssa"].contains(&extension.as_str()) {
        println!("  Processing {file_base}...");
    } else {
        println!("  Ignoring {file_base}...");
        return (filename_without_ext, None, false);
    }

    // Only do the TMDb API stuff if it's not preprocessed
    if !preprocessed {
        // Search using the TMDb API
        let year = metadata.year().map(|y| y as u16);
        let search = MovieSearch::new(metadata.title().to_string()).with_year(year);
        let reply = search.execute(tmdb).await;

        let results = match reply {
            Ok(res) => Ok(res.results),
            Err(e) => {
                eprintln!("  There was an error while searching {file_base}!");
                Err(e)
            }
        };

        let mut movie_list: Vec<MovieEntry> = Vec::new();
        // Create movie entry from the result
        if results.is_ok() {
            for result in results.unwrap() {
                let mut movie_details = MovieEntry::from(result);
                // Get director's name, if needed
                if pattern.contains("{director}") {
                    let credits_search = MovieCredits::new(movie_details.id);
                    let credits_reply = credits_search.execute(tmdb).await;
                    if credits_reply.is_ok() {
                        let mut crew = credits_reply.unwrap().crew;
                        // Only keep the director(s)
                        crew.retain(|x| x.job == *"Director");
                        if !crew.is_empty() {
                            let directors: Vec<String> =
                                crew.iter().map(|x| x.person.name.clone()).collect();
                            let mut directors_text = directors.join(", ");
                            if let Some(pos) = directors_text.rfind(',') {
                                directors_text.replace_range(pos..pos + 2, " and ");
                            }
                            movie_details.director = Some(directors_text);
                        }
                    }
                }
                movie_list.push(movie_details);
            }
        }

        // If nothing is found, skip
        if movie_list.is_empty() {
            eprintln!("  Could not find any entries matching {file_base}!");
            return (filename_without_ext, None, true);
        }

        let choice;
        if lucky {
            // Take first choice if in lucky mode
            choice = movie_list.into_iter().next().unwrap();
        } else {
            // Choose from the possible entries
            choice = match Select::new(
                format!("  Possible choices for {file_base}:").as_str(),
                movie_list,
            )
            .prompt()
            {
                Ok(movie) => movie,
                Err(error) => {
                    println!("  {error}");
                    let flag = matches!(
                        error,
                        InquireError::OperationCanceled | InquireError::OperationInterrupted
                    );
                    return (filename_without_ext, None, flag);
                }
            };
        };

        // Create the new name
        new_name_base = choice.rename_format(String::from(pattern));
    } else {
        println!("  Using previous choice for related files...");
    }

    // Handle the case for subtitle files
    if ["srt", "ssa"].contains(&extension.as_str()) {
        // Try to detect if there's already language info in the filename, else ask user to choose
        if filename_parts.len() >= 3 && filename_parts[1].len() == 2 {
            println!(
                "  Keeping language {} as detected in the subtitle file's extension...",
                get_long_lang(filename_parts[1])
            );
            extension = format!("{}.{}", filename_parts[1], extension);
        } else {
            let lang_list = Language::generate_list();
            let lang_choice =
                Select::new("  Choose the language for the subtitle file:", lang_list)
                    .prompt()
                    .expect("  Invalid choice!");
            if lang_choice.short != *"none" {
                extension = format!("{}.{}", lang_choice.short, extension);
            }
        }
    }

    // Add extension and stuff to the new name
    let mut new_name_with_ext = new_name_base.clone();
    if !extension.is_empty() {
        new_name_with_ext = format!("{new_name_with_ext}.{extension}");
    }
    let mut new_name = new_name_with_ext.clone();
    if !parent.is_empty() {
        new_name = format!("{parent}/{new_name}");
    }

    // Process the renaming
    if *filename == new_name {
        println!("  [file] '{file_base}' already has correct name.");
    } else {
        println!("  [file] '{file_base}' -> '{new_name_with_ext}'");
        // Only do the rename of --dry-run isn't passed
        if !dry_run {
            if !Path::new(new_name.as_str()).is_file() {
                fs::rename(filename, new_name.as_str()).expect("  Unable to rename file!");
            } else {
                eprintln!("  Destination file already exists, skipping...");
            }
        }
    }
    (filename_without_ext, Some(new_name_base), true)
}

// Function to process the passed arguments
pub fn process_args(mut args: Vec<String>) -> (Vec<String>, HashMap<&'static str, bool>) {
    // Remove the entry corresponding to the running process
    args.remove(0);
    let mut entries = Vec::new();
    let mut settings = HashMap::from([("dry_run", false), ("directory", false), ("lucky", false)]);
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("  The expected syntax is:");
                println!(
                    "  movie-rename <filename(s)> [-n|--dry-run] [-d|--directory] [-l|--i-feel-lucky] [-v|--version]"
                );
                println!(
                "  There needs to be a config file named config in the $XDG_CONFIG_HOME/movie-rename/ directory."
                );
                println!("  It should consist of two lines. The first line should have your TMDb API key.");
                println!(
                    "  The second line should have a pattern, that will be used for the rename."
                );
                println!("  In the pattern, the variables need to be enclosed in {{}}, the supported variables are `title`, `year` and `director`.");
                println!(
                "  Default pattern is `{{title}} ({{year}}) - {{director}}`. Extension is always kept."
                );
                println!("  Passing --directory or -d assumes that the arguments are directory names, which contain exactly one movie and optionally subtitles.");
                println!("  Passing --dry-run or -n does a dry tun and only prints out the new names, without actually doing anything.");
                println!(
                    "  You can join the short flags -d, -n and -l together (e.g. -dn or -dln)."
                );
                println!("  Passing --version or -v shows version and exits.");
                println!("  Pass --help to get this again.");
                exit(0);
            }
            "--version" | "-v" => {
                println!("movie-rename {VERSION}");
                exit(0);
            }
            "--dry-run" => {
                println!("Doing a dry run...");
                settings.entry("dry_run").and_modify(|x| *x = true);
            }
            "--i-feel-lucky" => {
                println!("Choosing the first option automatically...");
                settings.entry("lucky").and_modify(|x| *x = true);
            }
            "--directory" => {
                println!("Running in directory mode...");
                settings.entry("directory").and_modify(|x| *x = true);
            }
            other => {
                if other.starts_with('-') {
                    if !other.starts_with("--") && other.len() < 5 {
                        // Process short args
                        if other.contains('l') {
                            println!("Choosing the first option automatically...");
                            settings.entry("lucky").and_modify(|x| *x = true);
                        }
                        if other.contains('d') {
                            println!("Running in directory mode...");
                            settings.entry("directory").and_modify(|x| *x = true);
                        }
                        if other.contains('n') {
                            println!("Doing a dry run...");
                            settings.entry("dry_run").and_modify(|x| *x = true);
                        }
                    } else {
                        eprintln!("Unknown argument passed: {other}");
                        exit(1);
                    }
                } else {
                    entries.push(arg);
                }
            }
        }
    }
    (entries, settings)
}

// RenderConfig for the menu items
fn get_render_config() -> RenderConfig {
    let mut render_config = RenderConfig::default();
    render_config.option_index_prefix = IndexPrefix::Simple;

    render_config.error_message = render_config
        .error_message
        .with_prefix(Styled::new("❌").with_fg(Color::LightRed));

    render_config
}
