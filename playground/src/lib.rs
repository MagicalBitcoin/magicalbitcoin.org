use std::collections::HashMap;
use std::rc::Rc;
use std::str::FromStr;

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use rand::Rng;

use log::{debug, info};

use serde::{Deserialize, Serialize};

use clap::AppSettings;

use magical_bitcoin_wallet::bitcoin;
use magical_bitcoin_wallet::database::memory::MemoryDatabase;
use magical_bitcoin_wallet::miniscript;
use magical_bitcoin_wallet::descriptor::DescriptorExtendedKey;
use magical_bitcoin_wallet::descriptor::keys::{Key, parse_key};
use magical_bitcoin_wallet::descriptor::error::Error as DescriptorError;
use magical_bitcoin_wallet::*;

use bitcoin::*;
use bitcoin::secp256k1::Secp256k1;

use miniscript::policy::Concrete;
use miniscript::Descriptor;

mod utils;

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[wasm_bindgen]
pub fn init() {
    console_log::init_with_level(log::Level::Debug).unwrap();
    utils::set_panic_hook();

    info!("Initialization completed");
}

#[wasm_bindgen]
pub struct WalletWrapper {
    _change_descriptor: Option<String>,
    wallet: Rc<Wallet<EsploraBlockchain, MemoryDatabase>>,
}

#[wasm_bindgen]
impl WalletWrapper {
    #[wasm_bindgen(constructor)]
    pub async fn new(
        network: String,
        descriptor: String,
        change_descriptor: Option<String>,
        esplora: String,
    ) -> Result<WalletWrapper, String> {
        let network = match network.as_str() {
            "regtest" => Network::Regtest,
            "testnet" | _ => Network::Testnet,
        };

        debug!("descriptors: {:?} {:?}", descriptor, change_descriptor);

        let blockchain = EsploraBlockchain::new(&esplora);
        let wallet = Wallet::new(
            descriptor.as_str(),
            change_descriptor.as_ref().map(|x| x.as_str()),
            network,
            MemoryDatabase::new(),
            blockchain,
        )
        .await
        .map_err(|e| format!("{:?}", e))?;

        Ok(WalletWrapper {
            _change_descriptor: change_descriptor,
            wallet: Rc::new(wallet),
        })
    }

    #[wasm_bindgen]
    pub fn run(&self, line: String) -> Promise {
        let mut app = cli::make_cli_subcommands().setting(AppSettings::NoBinaryName);
        let wallet = Rc::clone(&self.wallet);

        future_to_promise(async move {
            let matches = app
                .get_matches_from_safe_borrow(line.split(" "))
                .map_err(|e| e.message)?;
            let res = cli::handle_matches(&wallet, matches)
                .await
                .map_err(|e| format!("{:?}", e))?;

            Ok(res.into())
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Alias {
    GenWif,
    GenExt { extra: String },
    Existing { extra: String },
}

impl Alias {
    fn into_key(self) -> String {
        match self {
            Alias::GenWif => {
                let key = secp256k1::key::SecretKey::new(&mut rand::thread_rng());
                let sk = bitcoin::util::key::PrivateKey {
                    compressed: true,
                    network: Network::Testnet,
                    key,
                };

                sk.to_wif()
            }
            Alias::GenExt { extra: path } => {
                let seed = rand::thread_rng().gen::<[u8; 32]>();
                let xprv =
                    bitcoin::util::bip32::ExtendedPrivKey::new_master(Network::Testnet, &seed)
                        .unwrap();

                format!("{}{}", xprv, path)
            }
            Alias::Existing { extra } => extra,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct CompilerResult {
    descriptor: String,
    aliases: HashMap<String, (String, String)>,
}

fn add_public_to_aliases(map: HashMap<String, String>) -> Result<HashMap<String, (String, String)>, String> {
	let secp = Secp256k1::gen_new();

    Ok(map
        .into_iter()
        .map(|(k, v)| {
			let (key, parsed) = parse_key(&v)?;
			Ok((k, (key, format!("{}", parsed.public(&secp)?))))
		})
        .collect::<Result<_, _>>().map_err(|e: DescriptorError| format!("{:?}", e))?)
}

#[wasm_bindgen]
pub fn compile(policy: String, aliases: String, script_type: String) -> Promise {
    future_to_promise(async move {
        let aliases: HashMap<String, Alias> =
            serde_json::from_str(&aliases).map_err(|e| format!("{:?}", e))?;
        let aliases: HashMap<String, String> = aliases
            .into_iter()
            .map(|(k, v)| (k, v.into_key()))
            .collect();

        let policy = Concrete::<String>::from_str(&policy).map_err(|e| format!("{:?}", e))?;
        let compiled = policy.compile().map_err(|e| format!("{:?}", e))?;

        let descriptor = match script_type.as_str() {
            "sh" => Descriptor::Sh(compiled),
            "wsh" => Descriptor::Wsh(compiled),
            "sh-wsh" => Descriptor::ShWsh(compiled),
            _ => return Err("InvalidScriptType".into()),
        };

        let descriptor: Result<Descriptor<String>, String> = descriptor.translate_pk(
            |key| Ok(aliases.get(key).unwrap_or(key).into()),
            |key| Ok(aliases.get(key).unwrap_or(key).into()),
        );
        let descriptor = descriptor?;

		let result = CompilerResult {
			descriptor: format!("{}", descriptor),
			aliases: add_public_to_aliases(aliases)?,
		};

		Ok(serde_json::to_string(&result).unwrap().into())
    })
}
