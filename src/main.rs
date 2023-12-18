use octocrab::{Octocrab, params};
use std::{
    fs,
    io::{prelude::*, BufReader},
    net::{TcpListener, TcpStream},
};
use changelog_v1::thread_pool::ThreadPool;

/**
 * Two approaches:
 *  - Tagging; using keywords analysis, NLP and an understanding of commits (e.g. files introduced will be related to those in the same commit)
 *  - Graphs; count the number of edges etc shared with those edited together.
 */

#[tokio::main]
async fn main() -> octocrab::Result<()> {
    // TODO: Get username and repo
    // let args: Vec<String> = env::args().collect();
    // dbg!(args);

    let reading_username = "XAMPPRocky";
    let reading_repo = "octocrab";

    let gh_token = std::env::var("GITHUB_TOKEN")
        .expect("GITHUB_TOKEN env variable is required");

    let gh_conn = Octocrab::builder()
        .personal_token(gh_token)
        .build()?;

    let page = gh_conn.pulls(reading_username, reading_repo).list()
        .state(params::State::Open)
        .head("main")
        .sort(params::pulls::Sort::Popularity)
        .direction(params::Direction::Ascending)
        .per_page(1)
        .page(1u32)
        // Send the request
        .send()
        .await?;

    // println!("Test {:?}", page);

    create_server();

    Ok(())
}

fn create_server() {
    let listener = TcpListener::bind("127.0.0.1:7878").unwrap();
    let pool = ThreadPool::new(4);

    for stream in listener.incoming() {
        let stream = stream.unwrap();

        pool.execute(|| {
            handle_connection(stream);
        });
    }
}

fn handle_connection(mut stream: TcpStream) {
    let buf_reader = BufReader::new(&mut stream);
    let http_request: Vec<_> = buf_reader
        .lines()
        .map(|result| result.unwrap())
        .take_while(|line| !line.is_empty())
        .collect();

    let status_line = "HTTP/1.1 200 OK";
    let contents = fs::read_to_string("hello.html").unwrap();
    let length = contents.len();

    let response =
        format!("{status_line}\r\nContent-Length: {length}\r\n\r\n{contents}");

    stream.write_all(response.as_bytes()).unwrap();
}