// Built-in uses
use std::fmt;
// External uses
use async_trait::async_trait;
use num::BigUint;
use serde::{Deserialize, Serialize};
// Workspace uses
use zksync::utils::closest_packable_token_amount;
use zksync_types::{tx::PackedEthSignature, TokenLike, ZkSyncTx};
// Local uses
use super::{Fees, Scenario, ScenarioResources};
use crate::{
    monitor::Monitor,
    utils::{foreach_failsafe, gwei_to_wei, wait_all_failsafe_chunks, CHUNK_SIZES},
    wallet::ScenarioWallet,
};

/// Configuration options for the transfers scenario.
#[derive(Debug, Serialize, Deserialize, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct TransferScenarioConfig {
    /// Amount of money to be used in the transfer, in gwei.
    pub transfer_size: u64,
    /// Amount of iterations to rotate funds, "length" of the test.
    pub transfer_rounds: u64,
    /// Amount of intermediate wallets to use.
    ///
    /// Due to scenario implementation details, amount of intermediate wallets
    /// should be greater than the expected block size.
    pub wallets_amount: u64,
}

impl Default for TransferScenarioConfig {
    fn default() -> Self {
        Self {
            transfer_size: 1,
            transfer_rounds: 10,
            wallets_amount: 100,
        }
    }
}

/// Schematically, scenario will look like this:
///
/// ```text
/// Deposit  | Transfer to new  | Transfer | Collect back | Withdraw to ETH
///          |                  |          |              |
///          |                  |          |              |
///          |           ┏━━━>Acc1━━━━━┓┗>Acc1━━━┓     |
///          |         ┏━┻━━━>Acc2━━━━┓┗━>Acc2━━━┻┓    |
/// ETH━━━━>InitialAcc━╋━━━━━>Acc3━━━┓┗━━>Acc3━━━━╋━>InitialAcc━>ETH
///          |         ┗━┳━━━>Acc4━━┓┗━━━>Acc4━━━┳┛    |
///          |           ┗━━━>Acc5━┓┗━━━━>Acc5━━━┛     |
/// ```
#[derive(Debug)]
pub struct TransferScenario {
    token_name: TokenLike,
    transfer_size: BigUint,
    transfer_rounds: u64,
    wallets: u64,
    txs: Vec<(ZkSyncTx, Option<PackedEthSignature>)>,
}

impl TransferScenario {
    pub fn new(token_name: TokenLike, config: TransferScenarioConfig) -> Self {
        Self {
            token_name,
            transfer_size: gwei_to_wei(config.transfer_size),
            transfer_rounds: config.transfer_rounds,
            wallets: config.wallets_amount,
            txs: Vec::new(),
        }
    }
}

impl fmt::Display for TransferScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transfers({})", self.token_name)
    }
}

#[async_trait]
impl Scenario for TransferScenario {
    fn requested_resources(&self, fees: &Fees) -> ScenarioResources {
        let balance_per_wallet =
            &self.transfer_size + (&fees.zksync * BigUint::from(self.transfer_rounds));

        ScenarioResources {
            balance_per_wallet,
            wallets_amount: self.wallets,
            token_name: self.token_name.clone(),
            has_deposits: false,
        }
    }

    async fn prepare(
        &mut self,
        _monitor: &Monitor,
        fees: &Fees,
        wallets: &[ScenarioWallet],
    ) -> anyhow::Result<()> {
        let transfers_number = (self.wallets * self.transfer_rounds) as usize;

        vlog::info!(
            "All the initial transfers have been verified, creating {} transactions \
            for the transfers step",
            transfers_number
        );

        self.txs = wait_all_failsafe_chunks(
            "prepare/transfers",
            CHUNK_SIZES,
            (0..transfers_number).map(|i| {
                let from = i % wallets.len();
                let to = (i + 1) % wallets.len();

                wallets[from].sign_transfer(
                    wallets[to].address(),
                    closest_packable_token_amount(&self.transfer_size),
                    fees.zksync.clone(),
                )
            }),
        )
        .await?;

        vlog::info!("Created {} transactions...", self.txs.len());

        Ok(())
    }

    async fn run(
        &mut self,
        monitor: Monitor,
        _fees: Fees,
        wallets: Vec<ScenarioWallet>,
    ) -> anyhow::Result<Vec<ScenarioWallet>> {
        foreach_failsafe(
            "run/transfers",
            self.txs
                .drain(..)
                .map(|(tx, sign)| monitor.send_tx(tx, sign)),
        )
        .await?;

        Ok(wallets)
    }

    async fn finalize(
        &mut self,
        _monitor: &Monitor,
        _fees: &Fees,
        _wallets: &[ScenarioWallet],
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
