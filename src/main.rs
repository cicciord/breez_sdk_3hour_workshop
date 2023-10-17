use bip39::{Language, Mnemonic};
use breez_sdk_core::InputType::LnUrlWithdraw;
use breez_sdk_core::ListPaymentsRequest;
use breez_sdk_core::Payment;
use breez_sdk_core::PaymentTypeFilter;
use breez_sdk_core::{
    parse, BreezEvent, BreezServices, EnvironmentType, EventListener, GreenlightNodeConfig,
    ReceivePaymentRequest, ReceivePaymentResponse,
};
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use log::info;
use serde::Serialize;
use std::io;
use std::io::prelude::*;
use std::sync::Arc;
use std::{env, str::FromStr};

#[tokio::main]
async fn main() {
    dotenv().ok();
    let cli = Cli::parse();
    stderrlog::new()
        .show_level(false)
        .modules(vec!["breez_sdk_3hour_workshop", "breez_sdk_core"])
        .verbosity(match cli.verbose {
            true => stderrlog::LogLevelNum::Debug,
            false => stderrlog::LogLevelNum::Info,
        })
        .init()
        .unwrap();
    match &cli.command {
        Commands::GenerateMnemonic => {
            let mnemonic = Mnemonic::generate_in(Language::English, 12).unwrap();
            info!("Generated mnemonic: {mnemonic}");
            info!("Set the environment variable 'MNEMONIC', and run another command.");
        }
        Commands::NodeInfo => {
            let sdk = connect().await;
            let node_info = sdk.node_info().unwrap();
            info!("Node ID: {:?}", node_info.id);
            info!("Spendable Amount: {:?}", node_info.max_payable_msat);
            pause();
        }
        Commands::ReceivePayment {
            amount_sats,
            description,
        } => {
            let sdk = connect().await;
            let invoice = receive_payment(&sdk, amount_sats, description).await;
            info!("Invoice created: {}", invoice.ln_invoice.bolt11);
            info!(
                "Expected opening fee (msat): {:?}",
                invoice.opening_fee_msat
            );
            info!("Waiting for invoice to be paid...");
            pause();
        }
        Commands::LnUrlWithdraw { lnurl } => {
            let sdk = connect().await;
            lnurl_withdraw(&sdk, &lnurl).await;
            pause();
        }
        Commands::LnUrlPay { lnurl } => {
            let sdk = connect().await;
            send_payment(&sdk, &lnurl).await;
            pause();
        }
        Commands::ListPayments => {
            let sdk = connect().await;
            let payments = list_payments(&sdk).await;
            dbg!("{:?}", payments);
            pause();
        }
        Commands::ChatGPT { prompt } => {
            let sdk = connect().await;

            let url = "http://178.21.114.20:8000/openai/v1/chat/completions";
            info!("Calling http 402 API without a token.");

            let client = reqwest::ClientBuilder::new().build().unwrap();
            let req = &GptRequest {
                model: String::from("gpt-3.5-turbo"),
                messages: vec![GptMessage {
                    role: String::from("user"),
                    content: prompt.clone(),
                }],
            };
            let mut resp = client.post(url).json(&req).send().await.unwrap();
            info!("Response status is {}", resp.status());
            let l402header = resp
                .headers()
                .get("WWW-Authenticate")
                .expect("server did not return WWW-Authenticate header in 402 response.")
                .to_str()
                .unwrap();

            info!("Got WWW-Authenticate header: {}", l402header);
            let re = regex::Regex::new(
                r#"^L402 (token|macaroon)=\"(?<token>.*)\", invoice=\"(?<invoice>.*)\""#,
            )
            .unwrap();
            let caps = re
                .captures(l402header)
                .expect("WWW-Authenticate header is not a valid L402");
            let token = caps["token"].to_string();
            let invoice = caps["invoice"].to_string();
            info!(
                "Got lightning invoice to get access to the API: {}",
                invoice
            );

            info!(
                "Paying lightning invoice to get access to the API: {}",
                invoice
            );
            let payresult = sdk.send_payment(invoice, None).await.unwrap();
            let lnpayresult = match payresult.details {
                breez_sdk_core::PaymentDetails::Ln { data } => data,
                _ => unreachable!(),
            };

            let header = format!("L402 {}:{}", token, lnpayresult.payment_preimage);
            info!(
                "Calling http 402 api again, now with header Authorization {}",
                header
            );
            resp = client
                .post(url)
                .header("Authorization", header)
                .json(&req)
                .send()
                .await
                .unwrap();

            let status = resp.status();
            info!("Got Response. Status {}", status);
            let text = resp.text().await.unwrap();
            info!("{}", text);
        }
    };
}

#[derive(Parser)]
#[command(name = "breez-sdk-demo")]
#[command(author = "Jesse de Wit <witdejesse@hotmail.com>")]
#[command(version = "0.1")]
#[command(about = "Example commandline application for the Breez SDK")]
#[command(long_about = None)]
struct Cli {
    #[arg(short, long, action)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[clap(alias = "mnemonic")]
    GenerateMnemonic,

    #[clap(alias = "nodeinfo")]
    NodeInfo,

    #[clap(alias = "receivepayment")]
    ReceivePayment {
        #[clap(long, short)]
        amount_sats: u64,
        #[clap(long, short)]
        description: String,
    },

    #[clap(alias = "lnurlwithdraw")]
    LnUrlWithdraw {
        #[clap(long, short)]
        lnurl: String,
    },

    #[clap(alias = "lnurlpay")]
    LnUrlPay {
        #[clap(long, short)]
        lnurl: String,
    },

    #[clap(alias = "listpayments")]
    ListPayments,

    #[clap(alias = "chatgpt")]
    ChatGPT {
        #[clap(long, short)]
        prompt: String,
    },
}

fn get_env_var(name: &str) -> Result<String, String> {
    let v = match env::var(name) {
        Ok(v) => v,
        Err(_) => return Err("variable not set".to_string()),
    };

    if v.is_empty() {
        return Err("variable is empty".to_string());
    }

    Ok(v)
}

fn pause() {
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();

    // We want the cursor to stay at the end of the line, so we print without a newline and flush manually.
    write!(stdout, "").unwrap();
    stdout.flush().unwrap();

    // Read a single byte and discard
    let _ = stdin.read(&mut [0u8]).unwrap();
}

#[derive(Serialize)]
pub struct GptRequest {
    pub model: String,
    pub messages: Vec<GptMessage>,
}

#[derive(Serialize)]
pub struct GptMessage {
    pub role: String,
    pub content: String,
}

struct AppEventListener {}

impl EventListener for AppEventListener {
    fn on_event(&self, e: breez_sdk_core::BreezEvent) {
        match e {
            BreezEvent::NewBlock { .. } => {}
            BreezEvent::InvoicePaid { .. } => {
                info!("Invoice paid!")
            }
            BreezEvent::Synced => {}
            BreezEvent::PaymentSucceed { .. } => {
                info!("Paymanet succeded!");
            }
            BreezEvent::PaymentFailed { .. } => {
                info!("Payment failed!");
            }
            BreezEvent::BackupStarted => {}
            BreezEvent::BackupSucceeded => {}
            BreezEvent::BackupFailed { .. } => {}
        }
    }
}

async fn connect() -> Arc<BreezServices> {
    let mnemonic_str = get_env_var("MNEMONIC").unwrap();
    let mnemonic = Mnemonic::from_str(&mnemonic_str).unwrap();
    let seed = mnemonic.to_seed("");
    let invite_code = Some(get_env_var("GREENLIGHT_INVITE_CODE").unwrap()).into();
    let api_key = get_env_var("BREEZ_API_KEY").unwrap().into();

    let mut config = BreezServices::default_config(
        EnvironmentType::Production,
        api_key,
        breez_sdk_core::NodeConfig::Greenlight {
            config: GreenlightNodeConfig {
                partner_credentials: None,
                invite_code,
            },
        },
    );

    config.exemptfee_msat = 50000;

    let sdk = BreezServices::connect(config, seed.to_vec(), Box::new(AppEventListener {}))
        .await
        .unwrap();

    sdk
}

async fn receive_payment(
    sdk: &Arc<BreezServices>,
    amount_sats: &u64,
    description: &str,
) -> ReceivePaymentResponse {
    sdk.receive_payment(ReceivePaymentRequest {
        amount_sats: *amount_sats,
        description: String::from_str(description).unwrap(),
        cltv: None,
        expiry: None,
        opening_fee_params: None,
        preimage: None,
        use_description_hash: None,
    })
    .await
    .unwrap()
}

async fn lnurl_withdraw(sdk: &Arc<BreezServices>, lnurl: &str) {
    let lsp_id = sdk.lsp_id().await.unwrap().unwrap();
    sdk.connect_lsp(lsp_id).await.unwrap();

    if let Ok(LnUrlWithdraw { data: wd }) = parse(lnurl).await {
        let amount_msat = wd.max_withdrawable;
        let description = "Test withdraw".to_string();

        let _ = sdk
            .lnurl_withdraw(wd, amount_msat / 1000, Some(description))
            .await
            .unwrap();
    }
}

async fn send_payment(sdk: &Arc<BreezServices>, bolt11: &str) -> Payment {
    sdk.send_payment(bolt11.into(), None).await.unwrap()
}

async fn list_payments(sdk: &Arc<BreezServices>) -> Vec<Payment> {
    sdk.list_payments(ListPaymentsRequest {
        filter: PaymentTypeFilter::All,
        from_timestamp: None,
        to_timestamp: None,
        include_failures: Some(true),
    })
    .await
    .unwrap()
}
