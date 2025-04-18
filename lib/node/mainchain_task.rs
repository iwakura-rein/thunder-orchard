//! Task to communicate with mainchain node

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bitcoin::{self, hashes::Hash as _};
use fallible_iterator::FallibleIterator;
use futures::{
    StreamExt,
    channel::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
};
use sneed::{EnvError, RwTxnError};
use thiserror::Error;
use tokio::{
    spawn,
    task::{self, JoinHandle},
};

use crate::{
    archive::{self, Archive},
    types::proto::{self, mainchain},
};

/// Request data from the mainchain node
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Request {
    /// Request missing mainchain ancestor headers
    AncestorHeaders(bitcoin::BlockHash),
    /// Request recursive BMM verification
    VerifyBmm(bitcoin::BlockHash),
}

/// Error included in a response
#[derive(Debug, Error)]
pub enum ResponseError {
    #[error("Archive error")]
    Archive(#[from] archive::Error),
    #[error("Database env error")]
    DbEnv(#[from] EnvError),
    #[error("Database write error")]
    DbWrite(#[from] sneed::rwtxn::Error),
    #[error("CUSF Mainchain proto error")]
    Mainchain(#[from] proto::Error),
}

/// Response indicating that a request has been fulfilled
#[derive(Debug)]
pub(super) enum Response {
    AncestorHeaders(bitcoin::BlockHash, Result<(), ResponseError>),
    VerifyBmm(
        bitcoin::BlockHash,
        Result<Result<(), mainchain::BlockNotFoundError>, ResponseError>,
    ),
}

impl From<&Response> for Request {
    fn from(resp: &Response) -> Self {
        match resp {
            Response::AncestorHeaders(block_hash, _) => {
                Request::AncestorHeaders(*block_hash)
            }
            Response::VerifyBmm(block_hash, _) => {
                Request::VerifyBmm(*block_hash)
            }
        }
    }
}

#[derive(Debug, Error)]
enum Error {
    #[error("Send response error")]
    SendResponse(Response),
    #[error("Send response error (oneshot)")]
    SendResponseOneshot(Response),
}

struct MainchainTask<Transport = tonic::transport::Channel> {
    env: sneed::Env,
    archive: Archive,
    mainchain: proto::mainchain::ValidatorClient<Transport>,
    // receive a request, and optional oneshot sender to send the result to
    // instead of sending on `response_tx`
    request_rx: UnboundedReceiver<(Request, Option<oneshot::Sender<Response>>)>,
    response_tx: UnboundedSender<Response>,
}

impl<Transport> MainchainTask<Transport>
where
    Transport: proto::Transport,
{
    /// Request ancestor headers from the mainchain node,
    /// including the specified header
    async fn request_ancestor_headers(
        env: &sneed::Env,
        archive: &Archive,
        cusf_mainchain: &mut proto::mainchain::ValidatorClient<Transport>,
        block_hash: bitcoin::BlockHash,
    ) -> Result<(), ResponseError> {
        if block_hash == bitcoin::BlockHash::all_zeros() {
            return Ok(());
        } else {
            let rotxn = env.read_txn().map_err(EnvError::from)?;
            if archive
                .try_get_main_header_info(&rotxn, block_hash)?
                .is_some()
            {
                return Ok(());
            }
        }
        let mut current_block_hash = block_hash;
        let mut current_height = None;
        let mut header_infos = Vec::<mainchain::BlockHeaderInfo>::new();
        tracing::debug!(%block_hash, "requesting ancestor headers");
        const LOG_PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
        const BATCH_REQUEST_SIZE: u32 = 1000;
        let mut progress_logged = Instant::now();
        loop {
            if let Some(current_height) = current_height {
                let now = Instant::now();
                if now.duration_since(progress_logged) >= LOG_PROGRESS_INTERVAL
                {
                    progress_logged = now;
                    tracing::debug!(
                        %block_hash,
                        "requesting ancestor headers: {current_block_hash}({current_height} remaining)");
                }
                tracing::trace!(%block_hash, "requesting ancestor headers: {current_block_hash}({current_height})")
            }
            let header_infos_resp = cusf_mainchain
                .get_block_header_infos(
                    current_block_hash,
                    BATCH_REQUEST_SIZE - 1,
                )
                .await?;
            {
                let current = header_infos_resp.last();
                current_block_hash = current.prev_block_hash;
                current_height = current.height.checked_sub(1);
            }
            header_infos.extend(header_infos_resp);
            if current_block_hash == bitcoin::BlockHash::all_zeros() {
                break;
            } else {
                let rotxn = env.read_txn().map_err(EnvError::from)?;
                if archive
                    .try_get_main_header_info(&rotxn, current_block_hash)?
                    .is_some()
                {
                    break;
                }
            }
        }
        header_infos.reverse();
        // Writing all headers during IBD can starve archive readers.
        tracing::trace!(%block_hash, "storing ancestor headers");
        task::block_in_place(|| {
            let mut rwtxn = env.write_txn().map_err(EnvError::from)?;
            header_infos.into_iter().try_for_each(|header| {
                archive.put_main_header_info(&mut rwtxn, &header)
            })?;
            rwtxn.commit().map_err(RwTxnError::from)?;
            tracing::trace!(%block_hash, "stored ancestor headers");
            Ok(())
        })
    }

    /// Request ancestor BMM commitments from the mainchain node,
    /// up to and including the specified block.
    /// Mainchain headers for the specified block and all ancestors MUST exist
    /// in the archive.
    async fn request_bmm_commitments(
        env: &sneed::Env,
        archive: &Archive,
        mainchain: &mut mainchain::ValidatorClient<Transport>,
        main_hash: bitcoin::BlockHash,
    ) -> Result<Result<(), mainchain::BlockNotFoundError>, ResponseError> {
        if main_hash == bitcoin::BlockHash::all_zeros() {
            return Ok(Ok(()));
        } else {
            let rotxn = env.read_txn().map_err(EnvError::from)?;
            if archive
                .try_get_main_bmm_commitment(&rotxn, main_hash)?
                .is_some()
            {
                return Ok(Ok(()));
            }
        }
        let mut missing_commitments: Vec<_> = {
            let rotxn = env.read_txn().map_err(EnvError::from)?;
            archive
                .main_ancestors(&rotxn, main_hash)
                .take_while(|block_hash| {
                    Ok(*block_hash != bitcoin::BlockHash::all_zeros()
                        && archive
                            .try_get_main_bmm_commitment(&rotxn, *block_hash)?
                            .is_none())
                })
                .collect()?
        };
        tracing::debug!(%main_hash, "requesting ancestor bmm commitments");
        const BATCH_REQUEST_SIZE: usize = 1000;
        while !missing_commitments.is_empty() {
            let len = missing_commitments.len();
            let max_ancestors = if len >= BATCH_REQUEST_SIZE {
                BATCH_REQUEST_SIZE - 1
            } else {
                len - 1
            };
            let batch_start_idx = len - 1 - max_ancestors;
            let batch_start = missing_commitments[batch_start_idx];
            let batch_end = *missing_commitments.last().unwrap();
            tracing::trace!(%main_hash,
                %batch_start,
                %batch_end,
                "requesting ancestor bmm commitments"
            );
            let commitments = match mainchain
                .get_bmm_hstar_commitments(batch_start, max_ancestors as u32)
                .await?
            {
                Ok(commitments) => commitments,
                Err(block_not_found) => return Ok(Err(block_not_found)),
            };
            if commitments.tail.len() < max_ancestors {
                // A commitment was missing, return an error
                let err = mainchain::BlockNotFoundError(
                    missing_commitments
                        [batch_start_idx + commitments.tail.len() + 1],
                );
                return Ok(Err(err));
            }
            tracing::trace!(%main_hash,
                %batch_start,
                %batch_end,
                "storing ancestor bmm commitments"
            );
            {
                let mut rwtxn = env.write_txn().map_err(EnvError::from)?;
                missing_commitments
                    .drain(batch_start_idx..)
                    .rev()
                    .zip(commitments.into_iter().rev())
                    .try_for_each(|(missing_commitment, commitment)| {
                        archive.put_main_bmm_commitment(
                            &mut rwtxn,
                            missing_commitment,
                            commitment,
                        )
                    })?;
                rwtxn.commit().map_err(RwTxnError::from)?;
            }
            tracing::trace!(%main_hash,
                %batch_start,
                %batch_end,
                "stored ancestor bmm commitments"
            );
        }
        Ok(Ok(()))
    }

    async fn run(mut self) -> Result<(), Error> {
        while let Some((request, response_tx)) = self.request_rx.next().await {
            match request {
                Request::AncestorHeaders(main_block_hash) => {
                    let res = Self::request_ancestor_headers(
                        &self.env,
                        &self.archive,
                        &mut self.mainchain,
                        main_block_hash,
                    )
                    .await;
                    let response =
                        Response::AncestorHeaders(main_block_hash, res);
                    if let Some(response_tx) = response_tx {
                        response_tx
                            .send(response)
                            .map_err(Error::SendResponseOneshot)?;
                    } else {
                        self.response_tx.unbounded_send(response).map_err(
                            |err| Error::SendResponse(err.into_inner()),
                        )?;
                    }
                }
                Request::VerifyBmm(block_hash) => {
                    let response = Response::VerifyBmm(
                        block_hash,
                        Self::request_bmm_commitments(
                            &self.env,
                            &self.archive,
                            &mut self.mainchain,
                            block_hash,
                        )
                        .await,
                    );
                    if let Some(response_tx) = response_tx {
                        response_tx
                            .send(response)
                            .map_err(Error::SendResponseOneshot)?;
                    } else {
                        self.response_tx.unbounded_send(response).map_err(
                            |err| Error::SendResponse(err.into_inner()),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Handle to the task to communicate with mainchain node.
/// Task is aborted on drop.
#[derive(Clone)]
pub(super) struct MainchainTaskHandle {
    task: Arc<JoinHandle<()>>,
    // send a request, and optional oneshot sender to receive the result on the
    // corresponding oneshot receiver
    request_tx:
        mpsc::UnboundedSender<(Request, Option<oneshot::Sender<Response>>)>,
}

impl MainchainTaskHandle {
    pub fn new<Transport>(
        env: sneed::Env,
        archive: Archive,
        mainchain: mainchain::ValidatorClient<Transport>,
    ) -> (Self, mpsc::UnboundedReceiver<Response>)
    where
        Transport: proto::Transport + Send + 'static,
        <Transport as tonic::client::GrpcService<tonic::body::BoxBody>>::Future:
            Send,
    {
        let (request_tx, request_rx) = mpsc::unbounded();
        let (response_tx, response_rx) = mpsc::unbounded();
        let task = MainchainTask {
            env,
            archive,
            mainchain,
            request_rx,
            response_tx,
        };
        let task = spawn(async move {
            if let Err(err) = task.run().await {
                let err = anyhow::Error::from(err);
                tracing::error!("Mainchain task error: {err:#}");
            }
        });
        let task_handle = MainchainTaskHandle {
            task: Arc::new(task),
            request_tx,
        };
        (task_handle, response_rx)
    }

    /// Send a request
    pub fn request(&self, request: Request) -> Result<(), Request> {
        self.request_tx
            .unbounded_send((request, None))
            .map_err(|err| {
                let (request, _) = err.into_inner();
                request
            })
    }

    /// Send a request, and receive the response on a oneshot receiver instead
    /// of the response stream
    pub fn request_oneshot(
        &self,
        request: Request,
    ) -> Result<oneshot::Receiver<Response>, Request> {
        let (oneshot_tx, oneshot_rx) = oneshot::channel();
        let () = self
            .request_tx
            .unbounded_send((request, Some(oneshot_tx)))
            .map_err(|err| {
                let (request, _) = err.into_inner();
                request
            })?;
        Ok(oneshot_rx)
    }
}

impl Drop for MainchainTaskHandle {
    // If only one reference exists (ie. within self), abort the net task.
    fn drop(&mut self) {
        // use `Arc::get_mut` since `Arc::into_inner` requires ownership of the
        // Arc, and cloning would increase the reference count
        if let Some(task) = Arc::get_mut(&mut self.task) {
            task.abort()
        }
    }
}
