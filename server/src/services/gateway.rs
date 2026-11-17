use connectrpc::{ConnectError, RequestContext, ServiceRequest, ServiceResult, ServiceStream};

use crate::AppState;
use crate::proto::backroom::v1::*;

pub struct GatewaySvc {
    #[allow(dead_code)]
    state: AppState,
}

impl GatewaySvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl GatewayService for GatewaySvc {
    async fn subscribe(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, SubscribeRequest>,
    ) -> ServiceResult<ServiceStream<SubscribeResponse>> {
        Err(ConnectError::unimplemented("Subscribe is not implemented"))
    }
}
