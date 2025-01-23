use crates_language_server::ls;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(ls::Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
