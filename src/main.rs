use tokio::net::{TcpListener, TcpStream};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use structopt::StructOpt;
use openssl::ssl::{SslMethod, SslAcceptor, SslContext, Ssl, SslStream, SSL_VERIFY_NONE};use openssl::x509::X509FileType;
use tokio::io::{copy, split, stdin as tokio_stdin, stdout as tokio_stdout, AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{rustls, TlsConnector, TlsAcceptor};

use std::io;
use std::net::ToSocketAddrs;
use std::sync::Arc;

#[derive(StructOpt)]
struct ProxyConfig {
    #[structopt(short = "c")]
    local_addr: String,
    #[structopt(short = "s")]
    remote_addr: String,
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

async fn handle_client(mut client: TcpStream, remote: String, message: String, cafile: PathBuf) {
    
    let mut root_store = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(cafile)? {
        root_store.add(cert?)?;
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_safe_defaults()
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    match TcpStream::connect(remote).await {
        Ok(mut outbound) => {
            let domain = ServerName::try_from(remote)?.to_owned();
            let mut tls = connector.connect(domain, outbound).await.unwrap();
            outbound.write_all(message.as_bytes()).await.unwrap();
            let (mut client_reader, mut client_writer) = split(outbound);
            let (mut stdin, mut stdout) = (tokio_stdin(), tokio_stdout());

            tokio::select! {
                res = copy(&mut client_reader, &mut stdout) => {
                    if let Err(e) = res {
                        eprintln!("Error copying from client to stdout: {}", e);
                    }
                },

                res = copy(&mut stdin, &mut client_writer) => {
                    if let Err(e) = res {
                        eprintln!("Error copying from stdin to client: {}", e);
                    }
                }
            }

            Ok(())

        }

        Err(e) => {
            eprintln!("Error connecting to remote server: {}", e);
        }
    }

}

#[tokio::main]
async fn main() -> io::Result<()> {

    let config = ProxyConfig::from_args();
    let local = config.local_addr.clone();
    let remote = config.remote_addr.clone();

    handle_server(local, remote).await

}
            

