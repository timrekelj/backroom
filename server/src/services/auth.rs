use connectrpc::{ConnectError, RequestContext, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::proto::backroom::v1::*;

pub struct AuthSvc {
    #[allow(dead_code)]
    state: AppState,
}

impl AuthSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl AuthService for AuthSvc {
    async fn sign_up(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, SignUpRequest>,
    ) -> ServiceResult<SignUpResponse> {
        Err(ConnectError::unimplemented("SignUp is not implemented"))
    }

    async fn login(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, LoginRequest>,
    ) -> ServiceResult<LoginResponse> {
        Err(ConnectError::unimplemented("Login is not implemented"))
    }

    async fn get_me(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, GetMeRequest>,
    ) -> ServiceResult<GetMeResponse> {
        Err(ConnectError::unimplemented("GetMe is not implemented"))
    }
}
