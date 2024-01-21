use std::collections::HashMap;
use std::sync::Mutex;
use octocrab::models;
use tokio::sync::OnceCell;
use lazy_static::lazy_static;
use octocrab::{Octocrab, params};
use petgraph::{graph::{NodeIndex, UnGraph}, visit::EdgeRef};
use serde::{Serialize, Deserialize};
use std::fs::{File, read_to_string};
use std::io::prelude::*;
use std::net::{TcpStream, TcpListener};
use tokio::time::Instant;

use changelog_v1::thread_pool::ThreadPool;

lazy_static! {
    static ref GRAPH: Mutex<UnGraph<String, ChangeEdge>> = Mutex::new(UnGraph::new_undirected());
    static ref FILE_TO_NODE: Mutex<HashMap<String, FileInfo>> = Mutex::new(HashMap::new());
}
static GITHUB: OnceCell<Octocrab> = OnceCell::const_new();
async fn get_github() -> &'static Octocrab {
    GITHUB.get_or_init(|| async {
        // TODO: Make optional (if repo is private)
        let gh_token = std::env::var("GITHUB_TOKEN")
            .expect("GITHUB_TOKEN env variable is required");
    
        Octocrab::builder()
            .personal_token(gh_token)
            .build()
            .expect("Cannot connect to repo")
    })
    .await
}

/**
 * Two approaches:
 *  - Tagging; using keywords analysis, NLP and an understanding of commits (e.g. files introduced will be related to those in the same commit)
 *  - Graphs; count the number of edges etc shared with those edited together.
 * 
 * Here, we are taking the Graph approach.
 */
#[tokio::main]
async fn main() -> octocrab::Result<()> {
    let octocrab = get_github().await;

    let owner = "XAMPPRocky"; // Replace with the repository owner's name
    let repo = "octocrab"; // Replace with the repository name

    let prs = octocrab.pulls(owner, repo)
        .list()
        .state(params::State::Closed)
        .per_page(100)
        .send()
        .await?
        .items;
    let n_prs = &prs.len();

    let start = Instant::now();
    for pr in prs {
        process_pr(pr, &octocrab, &owner, &repo).await;
    }
    let duration = start.elapsed();

    // print_graph_edges();
    // print_graph_commit_edges();

    println!("Time elapsed processing {:?} pr's is: {:?}", n_prs, duration);

    let graph = GRAPH.lock().unwrap();
    let serialized_graph = serialize_graph(&graph);
    let json = serde_json::to_string_pretty(&serialized_graph).expect("Error serializing graph");

    let mut file = File::create("graph.json").expect("Error creating file");
    file.write_all(json.as_bytes()).expect("Error writing to file");

    create_server();

    Ok(())
}

async fn process_pr(pr: models::pulls::PullRequest, octocrab: &Octocrab, owner: &str, repo: &str) {
    println!("PR #{}: {:?}", pr.number, pr.title);

    // Fetch the commits associated with the pull request
    let pr_commits_url = format!("https://api.github.com/repos/{}/{}/pulls/{}/commits", owner, repo, pr.number);
    let commits: Vec<models::commits::Commit> = octocrab.get(pr_commits_url, None::<&()>).await.expect("no commits?");

    for commit in commits {
        println!("\tCommit: {}", commit.sha);
        let commit_url = format!("https://api.github.com/repos/{}/{}/commits/{}", owner, repo, commit.sha);

        // Fetch the specific commit details
        let commit_detail: models::commits::Commit = octocrab.get(commit_url, None::<&()>).await.expect("no commit details?");
        if let Some(pr_files) = commit_detail.files.clone() {
            let mut graph = GRAPH.lock().unwrap();
            let mut file_to_node = FILE_TO_NODE.lock().unwrap();

            for file in &pr_files {
                println!("\t\tFile: {:#?}", file.filename);

                // Check if the file is known under a different (previous) name
                let _node_index = if let Some(prev_name) = &file.previous_filename {
                    if let Some(file_info) = file_to_node.get_mut(prev_name) {
                        file_info.update_name(file.filename.clone());
                        println!("File name changed from {:?} to {:?}", file.filename.clone(), prev_name);
                        file_info.node_index
                    } else {
                        create_new_file_node(&file.filename, &mut graph, &mut file_to_node)
                    }
                } else {
                    match file_to_node.get(&file.filename) {
                        Some(file_info) => file_info.node_index,
                        None => create_new_file_node(&file.filename, &mut graph, &mut file_to_node),
                    }
                };
            }

            let n_files_changed = pr_files.len();
            for (i, file) in pr_files.clone().into_iter().enumerate() {
                // Create and count shared edges
                let this_edited = file_to_node.get(&file.filename).unwrap();
                for j in (i + 1)..n_files_changed {
                    if i == j { continue } // safety: do not connect to self

                    let other_edited_commit = pr_files.get(j).expect("No file at j");
                    let other_edited = file_to_node.get(&other_edited_commit.filename).unwrap();
                    let edge = graph.find_edge(this_edited.node_index, other_edited.node_index);
                    match edge {
                        Some(e) => {
                            let existing_edge = graph.edge_weight_mut(e).unwrap();
                            if !existing_edge.pr_numbers.contains(&pr.number) {
                                existing_edge.add_pr(pr.number);
                            }
                            existing_edge.add_commit(commit.commit.message.to_string());
                            println!("Adding weight between {:?} - {:?}", other_edited_commit.filename, file.filename);
                        }
                        None => {
                            let new_edge = ChangeEdge::new(pr.number, commit.commit.message.to_string());
                            graph.add_edge(this_edited.node_index, other_edited.node_index, new_edge);
                            println!("Adding node between {:?} - {:?}", other_edited_commit.filename, file.filename);
                        }
                    }
                }
            }
        }
    }
}
fn create_new_file_node(filename: &str, graph: &mut UnGraph<String, ChangeEdge>, file_to_node: &mut HashMap<String, FileInfo>) -> NodeIndex {
    let real_file = filename.to_string();
    let new_index = graph.add_node(real_file);
    file_to_node.insert(filename.to_string(), FileInfo::new(filename.to_string(), new_index));
    new_index
}

fn print_graph_edges() {
    let graph = GRAPH.lock().unwrap();

    for edge in graph.edge_indices() {
        let (source_node, target_node) = graph.edge_endpoints(edge).unwrap();
        let edge_data = &graph[edge];

        let source_info = &graph[source_node];
        let target_info = &graph[target_node];

        println!(
            "Edge from {:?} to {:?} in PRs: {:?}",
            source_info,
            target_info,
            edge_data.pr_numbers
        );
    }
}
fn print_graph_commit_edges() {
    let graph = GRAPH.lock().unwrap();

    for edge in graph.edge_indices() {
        let (source_node, target_node) = graph.edge_endpoints(edge).unwrap();
        let edge_data = &graph[edge];

        let source_info = &graph[source_node];
        let target_info = &graph[target_node];

        println!(
            "Edge from {:?} to {:?} in Commits: {:?}",
            source_info,
            target_info,
            edge_data.commit_numbers
        );
    }
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
    let mut buffer = [0; 1024];
    stream.read(&mut buffer).unwrap();

    let request_line = String::from_utf8_lossy(&buffer);
    let request_line = request_line.lines().next().unwrap();
    let requested_file = request_line.split_whitespace().nth(1).unwrap();

    let (status_line, filename) = if requested_file == "/" {
        ("HTTP/1.1 200 OK", "hello.html")
    } else if requested_file.ends_with(".html") {
        ("HTTP/1.1 200 OK", &requested_file[1..])
    } else if requested_file.ends_with(".json") {
        ("HTTP/1.1 200 OK", &requested_file[1..])
    } else {
        ("HTTP/1.1 404 NOT FOUND", "404.html")
    };

    let contents = read_to_string(filename).unwrap_or_else(|_| String::from("File not found"));

    let content_type = if filename.ends_with(".html") {
        "Content-Type: text/html"
    } else if filename.ends_with(".json") {
        "Content-Type: application/json"
    } else {
        "Content-Type: text/plain"
    };

    let response = format!("{status_line}\r\n{content_type}\r\n\r\n{contents}");

    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}


#[derive(Default)]
struct FileInfo {
    current_name: String,
    previous_names: Vec<String>,
    node_index: NodeIndex,
}
impl FileInfo {
    fn new(name: String, node_index: NodeIndex) -> Self {
        FileInfo {
            current_name: name,
            previous_names: Vec::new(),
            node_index,
        }
    }

    fn update_name(&mut self, new_name: String) {
        if new_name != self.current_name {
            self.previous_names.push(self.current_name.clone());
            self.current_name = new_name;
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChangeEdge {
    pr_numbers: Vec<u64>,
    commit_numbers: Vec<String>,
}
impl ChangeEdge {
    fn new(pr_number: u64, commit_numbers: String) -> Self {
        ChangeEdge {
            pr_numbers: vec![pr_number],
            commit_numbers: vec![commit_numbers],
        }
    }

    fn add_commit(&mut self, commit_number: String) {
        if !self.commit_numbers.contains(&commit_number) {
            self.commit_numbers.push(commit_number);
        }
    }
    fn add_pr(&mut self, pr_number: u64) {
        if !self.pr_numbers.contains(&pr_number) {
            self.pr_numbers.push(pr_number);
        }
    }
}

/***
 * TODO:
 *  - Multithreading reading of PR's versus singlethreaded
 *  - UI for changelog list
 *  - UI for modules
 *      - with thresholds of edge weight
 *  - Key could be file path as we see when it has been changed (look at oldname), maybe value should be struct, keep names of old file paths.
 */

#[derive(Serialize, Deserialize)]
struct SerializableNode {
    id: usize,
    data: String,
}

#[derive(Serialize, Deserialize)]
struct SerializableEdge {
    source: usize,
    target: usize,
    data: ChangeEdge,
}

#[derive(Serialize, Deserialize)]
struct SerializableGraph {
    nodes: Vec<SerializableNode>,
    edges: Vec<SerializableEdge>,
}

fn serialize_graph(graph: &UnGraph<String, ChangeEdge>) -> SerializableGraph {
    let nodes: Vec<SerializableNode> = graph.node_indices()
        .map(|node| SerializableNode {
            id: node.index(),
            data: graph[node].clone(),
        })
        .collect();

    let edges: Vec<SerializableEdge> = graph.edge_references()
        .map(|edge| SerializableEdge {
            source: edge.source().index(),
            target: edge.target().index(),
            data: edge.weight().clone(),
        })
        .collect();

    SerializableGraph { nodes, edges }
}
