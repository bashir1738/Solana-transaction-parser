use std::{error::Error, fmt, fs};

use base64::Engine;
use clap::{Parser, ValueEnum};
use serde::Serialize;
use solana_message::VersionedMessage;
use solana_transaction::versioned::VersionedTransaction;

const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const MEMO_PROGRAM: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

#[derive(Debug, Parser)]
#[command(author, version, about = "Decode and explain Solana wire transactions")]
struct Cli {
    /// Base58/base64 transaction bytes, or a path when --file is used.
    input: String,

    /// Read the transaction bytes from a file instead of the input argument.
    #[arg(long)]
    file: bool,

    /// Input encoding. Auto-detection tries base58 first, then base64.
    #[arg(long, value_enum, default_value_t = Encoding::Auto)]
    encoding: Encoding,

    /// Emit machine-readable JSON instead of the human-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Encoding {
    Auto,
    Base58,
    Base64,
}

#[derive(Debug)]
enum ParseError {
    Input(String),
    Decode(String),
    Serialize(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(message) | Self::Decode(message) | Self::Serialize(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for ParseError {}

#[derive(Debug, Serialize)]
struct TransactionReport {
    format: &'static str,
    signatures: Vec<String>,
    fee_payer: Option<String>,
    recent_blockhash: String,
    required_signatures: u8,
    readonly_signed_accounts: u8,
    readonly_unsigned_accounts: u8,
    account_keys: Vec<AccountReport>,
    instructions: Vec<InstructionReport>,
    address_table_lookups: usize,
}

#[derive(Debug, Serialize)]
struct AccountReport {
    index: usize,
    address: String,
    signer: bool,
    writable: bool,
}

#[derive(Debug, Serialize)]
struct InstructionReport {
    index: usize,
    program: String,
    accounts: Vec<String>,
    data_base58: String,
    data_len: usize,
    summary: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let input = if cli.file {
        fs::read_to_string(&cli.input).map_err(|error| {
            ParseError::Input(format!("cannot read '{}': {error}", cli.input))
        })?
    } else {
        cli.input
    };
    let bytes = decode_input(input.trim(), cli.encoding)?;
    let transaction: VersionedTransaction = bincode::deserialize(&bytes)
        .map_err(|error| ParseError::Serialize(format!("invalid Solana transaction: {error}")))?;
    let report = build_report(&transaction)?;

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| ParseError::Serialize(error.to_string()))?
        );
    } else {
        print_report(&report);
    }
    Ok(())
}

fn decode_input(input: &str, encoding: Encoding) -> Result<Vec<u8>, ParseError> {
    if input.is_empty() {
        return Err(ParseError::Input("transaction input is empty".into()));
    }
    match encoding {
        Encoding::Base58 => bs58::decode(input)
            .into_vec()
            .map_err(|error| ParseError::Decode(format!("invalid base58 input: {error}"))),
        Encoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(input)
            .map_err(|error| ParseError::Decode(format!("invalid base64 input: {error}"))),
        Encoding::Auto => bs58::decode(input).into_vec().or_else(|base58_error| {
            base64::engine::general_purpose::STANDARD
                .decode(input)
                .map_err(|base64_error| {
                    ParseError::Decode(format!(
                        "input is neither valid base58 ({base58_error}) nor base64 ({base64_error})"
                    ))
                })
        }),
    }
}

fn build_report(transaction: &VersionedTransaction) -> Result<TransactionReport, ParseError> {
    let message = &transaction.message;
    let static_keys = message.static_account_keys();
    let account_keys = static_keys
        .iter()
        .enumerate()
        .map(|(index, address)| AccountReport {
            index,
            address: address.to_string(),
            signer: message.is_signer(index),
            writable: message.is_maybe_writable(index),
        })
        .collect::<Vec<_>>();

    let instructions = message
        .instructions()
        .iter()
        .enumerate()
        .map(|(index, instruction)| {
            let program = static_keys
                .get(instruction.program_id_index as usize)
                .ok_or_else(|| ParseError::Decode(format!("instruction {index} has an invalid program index")))?;
            let accounts = instruction
                .accounts
                .iter()
                .map(|account_index| {
                    static_keys
                        .get(*account_index as usize)
                        .map(ToString::to_string)
                        .ok_or_else(|| ParseError::Decode(format!("instruction {index} has an invalid account index")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(InstructionReport {
                index,
                program: program.to_string(),
                accounts,
                data_base58: bs58::encode(&instruction.data).into_string(),
                data_len: instruction.data.len(),
                summary: summarize_instruction(&program.to_string(), &instruction.data),
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;

    Ok(TransactionReport {
        format: match message {
            VersionedMessage::Legacy(_) => "legacy",
            VersionedMessage::V0(_) => "v0",
            VersionedMessage::V1(_) => "v1",
        },
        signatures: transaction.signatures.iter().map(ToString::to_string).collect(),
        fee_payer: static_keys.first().map(ToString::to_string),
        recent_blockhash: message.recent_blockhash().to_string(),
        required_signatures: message.header().num_required_signatures,
        readonly_signed_accounts: message.header().num_readonly_signed_accounts,
        readonly_unsigned_accounts: message.header().num_readonly_unsigned_accounts,
        account_keys,
        instructions,
        address_table_lookups: message.address_table_lookups().map_or(0, <[_]>::len),
    })
}

fn summarize_instruction(program: &str, data: &[u8]) -> String {
    if program == SYSTEM_PROGRAM && data.len() >= 4 {
        return match u32::from_le_bytes(data[..4].try_into().unwrap()) {
            0 => "system: create account".into(),
            2 if data.len() >= 12 => format!(
                "system: transfer {} lamports",
                u64::from_le_bytes(data[4..12].try_into().unwrap())
            ),
            8 => "system: allocate".into(),
            9 => "system: assign".into(),
            _ => "system: instruction".into(),
        };
    }
    if (program == TOKEN_PROGRAM || program == TOKEN_2022_PROGRAM) && !data.is_empty() {
        return format!("token: {}", token_instruction_name(data[0]));
    }
    if program == MEMO_PROGRAM {
        return format!("memo: {}", String::from_utf8_lossy(data));
    }
    "unknown program instruction".into()
}

fn token_instruction_name(tag: u8) -> &'static str {
    match tag {
        0 => "initialize mint",
        1 => "initialize account",
        3 => "transfer",
        7 => "mint to",
        8 => "burn",
        9 => "close account",
        12 => "transfer checked",
        13 => "approve checked",
        14 => "burn checked",
        15 => "close account",
        _ => "instruction",
    }
}

fn print_report(report: &TransactionReport) {
    println!("Transaction ({})", report.format);
    println!("  signatures: {}", report.signatures.len());
    for signature in &report.signatures {
        println!("    {signature}");
    }
    println!("  fee payer: {}", report.fee_payer.as_deref().unwrap_or("unknown"));
    println!("  recent blockhash: {}", report.recent_blockhash);
    println!("  accounts: {}", report.account_keys.len());
    for account in &report.account_keys {
        println!(
            "    [{:>2}] {}{}{}",
            account.index,
            account.address,
            if account.signer { " signer" } else { "" },
            if account.writable { " writable" } else { " readonly" }
        );
    }
    println!("  instructions: {}", report.instructions.len());
    for instruction in &report.instructions {
        println!(
            "    [{}] {} | {} | {} bytes",
            instruction.index, instruction.program, instruction.summary, instruction.data_len
        );
        if !instruction.accounts.is_empty() {
            println!("        accounts: {}", instruction.accounts.join(", "));
        }
    }
    if report.address_table_lookups > 0 {
        println!("  address table lookups: {}", report.address_table_lookups);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_message::legacy::Message;
    use solana_transaction::Transaction;

    #[test]
    fn reports_system_transfer() {
        let payer = solana_keypair::Keypair::new();
        let recipient = solana_address::Address::new_unique();
        let instruction = solana_system_interface::instruction::transfer(&payer.pubkey(), &recipient, 42);
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let transaction = VersionedTransaction {
            signatures: vec![solana_signature::Signature::default()],
            message: VersionedMessage::Legacy(message),
        };
        let report = build_report(&transaction).unwrap();
        assert_eq!(report.format, "legacy");
        assert_eq!(report.instructions[0].summary, "system: transfer 42 lamports");
        assert_eq!(report.account_keys.len(), 3);
    }
}
