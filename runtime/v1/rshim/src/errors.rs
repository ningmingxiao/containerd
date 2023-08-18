pub enum ShimError {
    TTRPCError(ttrpc::error::Error),
    AnyhowError(anyhow::Error),
}

impl From<anyhow::Error> for ShimError {
    fn from(e: anyhow::Error) -> Self {
        Self::AnyhowError(e)
    }
}

impl From<ttrpc::error::Error> for ShimError {
    fn from(e: ttrpc::error::Error) -> Self {
        Self::TTRPCError(e)
    }
}

pub fn ttrpc_error(code: ttrpc::Code, message: String) -> ttrpc::Error {
    ttrpc::Error::RpcStatus(ttrpc::get_status(code, message))
}

impl From<ShimError> for ttrpc::Error {
    fn from(e: ShimError) -> Self {
        match e {
            ShimError::TTRPCError(e) => e,
            ShimError::AnyhowError(ref e) => ttrpc_error(ttrpc::Code::INTERNAL, format!("{:#}", e)),
        }
    }
}

impl std::fmt::Debug for ShimError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            ShimError::TTRPCError(ref e) => e.fmt(f),
            ShimError::AnyhowError(ref e) => std::fmt::Debug::fmt(&e, f),
        }
    }
}

pub type Result<T> = std::result::Result<T, ShimError>;
