use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::OnceCell;
use lazy_static::lazy_static;
use octocrab::{Octocrab, params, models::pulls::PullRequest, pulls::PullRequestHandler};
use petgraph::graph::{NodeIndex, UnGraph};

use changelog_v1::thread_pool::ThreadPool;

lazy_static! {
    static ref GRAPH: Mutex<UnGraph<String, PrEdge>> = Mutex::new(UnGraph::new_undirected());
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
    // TODO: Get username and repo
    // let args: Vec<String> = env::args().collect();
    // dbg!(args);

    let reading_username = "XAMPPRocky";
    let reading_repo = "octocrab";

    let pr_handler = get_github().await.pulls(reading_username, reading_repo);

    let page = pr_handler.list()
        .state(params::State::Closed)
        .head("main")
        // .per_page(4)
        // .page(1u32)
        // Send the request
        .send()
        .await?;

    for pr in page.into_iter() {
        add_nodes(pr, &pr_handler).await;
    }
    
    print_graph_edges();

    // create_server();

    Ok(())
}

async fn add_nodes(pr: PullRequest, pr_handler: &PullRequestHandler<'_>) {
    let pr_files = match pr_handler.list_files(pr.number).await {
        Ok(files) => files,
        Err(_) => {
            eprintln!("Failed to list files for PR #{}", pr.number);
            return;
        }
    };

    let title = pr.title.clone().unwrap_or_default();
    println!("Adding nodes for #{}: {}", pr.number, title);

    let mut graph = GRAPH.lock().unwrap();
    let mut file_to_node = FILE_TO_NODE.lock().unwrap();

    // Get or create nodes for files edited within this PR
    for edited_file in &pr_files.items {
        let filename = &edited_file.filename;
        let previous_filename = edited_file.previous_filename.as_ref();

        // Check if the file is known under a different (previous) name
        let _node_index = if let Some(prev_name) = previous_filename {
            if let Some(file_info) = file_to_node.get_mut(prev_name) {
                file_info.update_name(filename.clone());
                println!("File name changed from {:?} to {:?}", filename.clone(), prev_name);
                file_info.node_index
            } else {
                create_new_file_node(filename, &mut graph, &mut file_to_node)
            }
        } else {
            match file_to_node.get(filename) {
                Some(file_info) => file_info.node_index,
                None => create_new_file_node(filename, &mut graph, &mut file_to_node),
            }
        };
    }

    // Create and count shared edges
    let n_files_changed = pr_files.items.len();
    for (i, f) in pr_files.clone().into_iter().enumerate() {

        let fi = file_to_node.get(&f.filename).unwrap();

        for j in (i + 1)..n_files_changed {
            let g = pr_files.items.get(j).expect("No file at j");

            let gi = file_to_node.get(&g.filename).unwrap();

            let edge = graph.find_edge(fi.node_index, gi.node_index);
            match edge {
                Some(e) => {
                    let edge_data = graph.edge_weight_mut(e).unwrap();
                    edge_data.add_pr(pr.number);
                    println!("Adding weight between {:?} - {:?}", g.filename, f.filename);
                }
                None => {
                    let new_edge = PrEdge::new(pr.number);
                    graph.add_edge(fi.node_index, gi.node_index, new_edge);
                    // println!("Adding node between {:?} - {:?}", g.filename, f.filename);
                }
            }
        }
    }
}

fn create_new_file_node(filename: &str, graph: &mut UnGraph<String, PrEdge>, file_to_node: &mut HashMap<String, FileInfo>) -> NodeIndex {
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
/*
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
*/


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

pub struct PrEdge {
    pr_numbers: Vec<u64>,
}
impl PrEdge {
    fn new(pr_number: u64) -> Self {
        PrEdge {
            pr_numbers: vec![pr_number],
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
