use connectrpc::{ConnectError, RequestContext, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::proto::backroom::v1::*;

pub struct MessageSvc {
    #[allow(dead_code)]
    state: AppState,
}

impl MessageSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl MessageService for MessageSvc {
    async fn send_message(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, SendMessageRequest>,
    ) -> ServiceResult<SendMessageResponse> {
        Err(ConnectError::unimplemented("SendMessage is not implemented"))
    }

    async fn list_messages(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListMessagesRequest>,
    ) -> ServiceResult<ListMessagesResponse> {
        Err(ConnectError::unimplemented("ListMessages is not implemented"))
    }
}
