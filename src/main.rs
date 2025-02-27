use tokio::net::{TcpListener, TcpStream};
use tokio::io::{self};
use structopt::StructOpt;

#[derive(StructOpt)]
struct ProxyConfig {
    #[structopt(short, long)]
    local_addr: String,
    #[structopt(short, long)]
    remote_addr: String,
    #[structopt(short = "lp")]
    local_port: u16,
    #[structopt(short = "rp")]
    remote_port: u16,
}


async fn handle_server(local: String, remote: String) -> io::Result<()> {

    let listener = TcpListener::bind(local.clone()).await?;
    println!("Listening on: {}", local);

    loop {
        let (socket, _) = listener.accept().await?;
        let remote = remote.clone();
        tokio::spawn(async move {
            handle_client(socket, remote).await;
        });
    }
}

async fn handle_client(mut client: TcpStream, remote: String) {

    match TcpStream::connect(remote).await {
        Ok(mut outbound) => {
            let (mut ri, mut wi) = client.split();
            let (mut ro, mut wo) = outbound.split();

            let client_to_remote = io::copy(&mut ri, &mut wo);
            let remote_to_client = io::copy(&mut ro, &mut wi);

            tokio::select! {
                _ = client_to_remote => {},
                _ = remote_to_client => {},
            }
        }

        Err(e) => {
            eprintln!("Error connecting to remote server: {}", e);
        }
    }

}

#[tokio::main]
async fn main() -> io::Result<()> {

    let config = ProxyConfig::from_args();
    let local = format!("{}:{}", config.local_addr, config.local_port);
    let remote = format!("{}:{}", config.remote_addr, config.remote_port);

    handle_server(local, remote).await

}
            

