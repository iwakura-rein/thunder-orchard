//! RPC API

mod schema;

pub mod open_api {
    use jsonrpsee::{core::RpcResult, proc_macros::rpc};
    use l2l_openapi::open_api;

    use crate::schema;

    #[open_api]
    #[rpc(client, server)]
    pub trait Rpc {
        /// Get OpenAPI schema
        #[open_api_method(output_schema(PartialSchema = "schema::OpenApi"))]
        #[method(name = "openapi_schema")]
        async fn openapi_schema(&self) -> RpcResult<utoipa::openapi::OpenApi>;
    }
}

pub mod node {
    use std::{collections::HashSet, net::SocketAddr};

    use jsonrpsee::{core::RpcResult, proc_macros::rpc};
    use l2l_openapi::open_api;
    use serde::{Deserialize, Serialize};
    use thunder_orchard_types::{
        Block, BlockHash, MerkleRoot, OutPoint, Output, OutputContent, Pointed,
        PointedOutput, SpentOutput, Transaction, TransparentAddress, Txid,
        WithdrawalBundle, net::Peer, schema as thunder_orchard_schema,
        transaction,
    };
    use utoipa::ToSchema;

    use crate::{open_api, schema};

    #[open_api(ref_schemas[Txid])]
    #[rpc(client, server, server_bounds(Self: open_api::RpcServer))]
    pub trait PrivateRpc {
        /// Connect to a peer
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "connect_peer")]
        async fn connect_peer(
            &self,
            #[open_api_method_arg(schema(
                PartialSchema = "thunder_orchard_schema::SocketAddr"
            ))]
            addr: SocketAddr,
        ) -> RpcResult<()>;

        /// Delete peer from known_peers DB.
        /// Connections to the peer are not terminated.
        #[method(name = "forget_peer")]
        async fn forget_peer(
            &self,
            #[open_api_method_arg(schema(
                PartialSchema = "thunder_orchard_schema::SocketAddr"
            ))]
            addr: SocketAddr,
        ) -> RpcResult<()>;

        /// Remove a tx from the mempool
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "remove_from_mempool")]
        async fn remove_from_mempool(&self, txid: Txid) -> RpcResult<()>;

        /// Stop the node
        #[method(name = "stop")]
        async fn stop(&self);
    }

    #[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
    pub struct GetTransactionResponse {
        pub tx: Transaction,
        /// Block hash, if in the active chain
        pub block_hash: Option<BlockHash>,
    }

    #[open_api(ref_schemas[
        MerkleRoot, OutPoint, Output, OutputContent, TransparentAddress, Txid,
        schema::BitcoinTxid, thunder_orchard_schema::BitcoinAddr,
        thunder_orchard_schema::BitcoinOutPoint,
    ])]
    #[rpc(client, server, server_bounds(Self: open_api::RpcServer))]
    pub trait Rpc {
        /// Connect a block template for which a BMM request was included in the
        /// specified mainchain block. Returns `true` if it was accepted as the new
        /// tip.
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "connect_block")]
        async fn connect_block(
            &self,
            block: Block,
            #[open_api_method_arg(schema(
                PartialSchema = "thunder_orchard_schema::BitcoinBlockHash"
            ))]
            main_block_hash: bitcoin::BlockHash,
        ) -> RpcResult<bool>;

        /// Get the best mainchain block hash known by Thunder-Orchard
        #[open_api_method(output_schema(
            PartialSchema = "schema::Optional<thunder_orchard_schema::BitcoinBlockHash>"
        ))]
        #[method(name = "get_best_mainchain_block_hash")]
        async fn get_best_mainchain_block_hash(
            &self,
        ) -> RpcResult<Option<bitcoin::BlockHash>>;

        /// Get the best sidechain block hash known by Thunder-Orchard
        #[open_api_method(output_schema(
            PartialSchema = "schema::Optional<thunder_orchard_types::BlockHash>"
        ))]
        #[method(name = "get_best_sidechain_block_hash")]
        async fn get_best_sidechain_block_hash(
            &self,
        ) -> RpcResult<Option<thunder_orchard_types::BlockHash>>;

        /// Get the block with specified block hash, if it exists
        #[method(name = "get_block")]
        async fn get_block(
            &self,
            block_hash: thunder_orchard_types::BlockHash,
        ) -> RpcResult<Option<thunder_orchard_types::Block>>;

        /// Get mainchain blocks that commit to a specified block hash
        #[open_api_method(output_schema(
            PartialSchema = "thunder_orchard_schema::BitcoinBlockHash"
        ))]
        #[method(name = "get_bmm_inclusions")]
        async fn get_bmm_inclusions(
            &self,
            block_hash: thunder_orchard_types::BlockHash,
        ) -> RpcResult<Vec<bitcoin::BlockHash>>;

        /// Get stxos for addresses
        #[method(name = "get_stxos")]
        async fn get_stxos(
            &self,
            addresses: HashSet<TransparentAddress>,
        ) -> RpcResult<Vec<Pointed<SpentOutput>>>;

        /// Get transaction by txid
        #[method(name = "get_transaction")]
        async fn get_transaction(
            &self,
            txid: Txid,
        ) -> RpcResult<Option<GetTransactionResponse>>;

        /// Get utxos for addresses
        #[method(name = "get_utxos")]
        async fn get_utxos(
            &self,
            addresses: HashSet<TransparentAddress>,
        ) -> RpcResult<Vec<PointedOutput>>;

        /// Get the current block count
        #[method(name = "getblockcount")]
        async fn getblockcount(&self) -> RpcResult<u32>;

        /// Get the height of the latest failed withdrawal bundle
        #[method(name = "latest_failed_withdrawal_bundle_height")]
        async fn latest_failed_withdrawal_bundle_height(
            &self,
        ) -> RpcResult<Option<u32>>;

        /// List peers
        #[method(name = "list_peers")]
        async fn list_peers(&self) -> RpcResult<Vec<Peer>>;

        /// List all UTXOs
        #[method(name = "list_utxos")]
        async fn list_utxos(&self) -> RpcResult<Vec<PointedOutput>>;

        /// Get pending withdrawal bundle
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "pending_withdrawal_bundle")]
        async fn pending_withdrawal_bundle(
            &self,
        ) -> RpcResult<Option<WithdrawalBundle>>;

        /// Get total sidechain wealth
        #[method(name = "sidechain_wealth")]
        async fn sidechain_wealth_sats(&self) -> RpcResult<u64>;

        /// Verify and broadcast a transaction
        #[method(name = "submit_transaction")]
        async fn submit_transaction(
            &self,
            transaction: transaction::Authorized<Transaction>,
        ) -> RpcResult<Txid>;
    }
}

pub mod wallet {
    use jsonrpsee::{core::RpcResult, proc_macros::rpc};
    use l2l_openapi::open_api;
    use serde::{Deserialize, Serialize};
    use thunder_orchard_types::{
        Block, BlockHash, MerkleRoot, OutPoint, Output, OutputContent,
        PointedOutput, ShieldedAddress, SpentOutput, Transaction,
        TransparentAddress, Txid, schema as thunder_orchard_schema,
        transaction, wallet::Balance,
    };
    use utoipa::ToSchema;

    use crate::{open_api, schema};

    #[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
    pub struct GetBlockTemplateResponse {
        /// Block hash to commit to in a BMM request
        pub critical_hash: BlockHash,
        /// Block to pass to `connect_block` once its BMM request is included in a
        /// mainchain block
        pub block: Block,
        /// Fees collected by the transactions in the block, in sats
        pub fees_sats: u64,
    }

    #[open_api(ref_schemas[
        MerkleRoot, OutPoint, Output, OutputContent, TransparentAddress, Txid,
        schema::BitcoinTxid, thunder_orchard_schema::BitcoinAddr,
        thunder_orchard_schema::BitcoinOutPoint,
    ])]
    #[rpc(client, server, server_bounds(Self: open_api::RpcServer))]
    pub trait Rpc {
        /// Get balance in sats
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "balance")]
        async fn balance(&self) -> RpcResult<Balance>;

        /// Deposit to address
        #[open_api_method(output_schema(
            PartialSchema = "schema::BitcoinTxid"
        ))]
        #[method(name = "create_deposit")]
        async fn create_deposit(
            &self,
            address: TransparentAddress,
            value_sats: u64,
            fee_sats: u64,
        ) -> RpcResult<bitcoin::Txid>;

        /// Create a tx that shields transparent funds
        #[method(name = "create_shield")]
        async fn create_shield(
            &self,
            value_sats: u64,
            fee_sats: u64,
        ) -> RpcResult<Txid>;

        /// Create a tx that transfers shielded funds to the specified address
        #[method(name = "create_shielded_transfer")]
        async fn create_shielded_transfer(
            &self,
            dest: ShieldedAddress,
            value_sats: u64,
            fee_sats: u64,
        ) -> RpcResult<Txid>;

        /// Create a tx that transfers funds to the specified address
        /// transparently
        #[method(name = "create_transparent_transfer")]
        async fn create_transparent_transfer(
            &self,
            dest: TransparentAddress,
            value_sats: u64,
            fee_sats: u64,
        ) -> RpcResult<Txid>;

        /// Create a tx that unshields shielded funds
        #[method(name = "create_unshield")]
        async fn create_unshield(
            &self,
            value_sats: u64,
            fee_sats: u64,
        ) -> RpcResult<Txid>;

        /// Creates a tx that initiates a withdrawal to the specified mainchain
        /// address
        #[method(name = "create_withdrawal")]
        async fn create_withdrawal(
            &self,
            #[open_api_method_arg(schema(
                PartialSchema = "thunder_orchard_schema::BitcoinAddr"
            ))]
            mainchain_address: bitcoin::Address<
                bitcoin::address::NetworkUnchecked,
            >,
            amount_sats: u64,
            fee_sats: u64,
            mainchain_fee_sats: u64,
        ) -> RpcResult<Txid>;

        /// Format a deposit address
        #[method(name = "format_deposit_address")]
        async fn format_deposit_address(
            &self,
            address: TransparentAddress,
        ) -> RpcResult<String>;

        /// Generate a mnemonic seed phrase
        #[method(name = "generate_mnemonic")]
        async fn generate_mnemonic(&self) -> RpcResult<String>;

        /// Assemble a block to blind merge mine, without requesting BMM for it.
        /// The caller requests BMM for `critical_hash` itself, then passes the
        /// block back to `connect_block`.
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "get_block_template")]
        async fn get_block_template(
            &self,
        ) -> RpcResult<GetBlockTemplateResponse>;

        /// Get a new shielded address
        #[method(name = "get_new_shielded_address")]
        async fn get_new_shielded_address(&self) -> RpcResult<ShieldedAddress>;

        /// Get a new transparent address
        #[method(name = "get_new_transparent_address")]
        async fn get_new_transparent_address(
            &self,
        ) -> RpcResult<TransparentAddress>;

        /// Get shielded wallet addresses, sorted by bech32m encoding
        #[method(name = "get_shielded_wallet_addresses")]
        async fn get_shielded_wallet_addresses(
            &self,
        ) -> RpcResult<Vec<thunder_orchard_types::orchard::Address>>;

        /// Get transparent wallet addresses, sorted by base58 encoding
        #[method(name = "get_transparent_wallet_addresses")]
        async fn get_transparent_wallet_addresses(
            &self,
        ) -> RpcResult<Vec<TransparentAddress>>;

        /// Get wallet UTXOs
        #[method(name = "get_wallet_utxos")]
        async fn get_wallet_utxos(&self) -> RpcResult<Vec<PointedOutput>>;

        /// Get wallet STXOs
        #[method(name = "get_wallet_stxos")]
        async fn get_wallet_stxos(
            &self,
        ) -> RpcResult<Vec<SpentOutput<PointedOutput>>>;

        /// Get unconfirmed wallet STXOs
        #[method(name = "get_wallet_stxos_unconfirmed")]
        async fn get_wallet_stxos_unconfirmed(
            &self,
        ) -> RpcResult<Vec<SpentOutput<PointedOutput>>>;

        /// Get unconfirmed wallet UTXOs
        #[method(name = "get_wallet_utxos_unconfirmed")]
        async fn get_wallet_utxos_unconfirmed(
            &self,
        ) -> RpcResult<Vec<PointedOutput>>;

        /// Attempt to mine a sidechain block
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "mine")]
        async fn mine(&self, fee: Option<u64>) -> RpcResult<()>;

        /// Set the wallet seed from a mnemonic seed phrase
        #[open_api_method(output_schema(ToSchema))]
        #[method(name = "set_seed_from_mnemonic")]
        async fn set_seed_from_mnemonic(
            &self,
            mnemonic: String,
        ) -> RpcResult<()>;

        /// Sign a transaction, and optionally broadcast it.
        #[method(name = "sign_transaction")]
        async fn sign_transaction(
            &self,
            transaction: Transaction,
            broadcast: Option<bool>,
        ) -> RpcResult<transaction::Authorized<Transaction>>;
    }
}
