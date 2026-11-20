//! Placeholder implementation of the admin service; every method returns
//! `unimplemented` until the feature is built.

use connectrpc::{ConnectError, RequestContext, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::proto::thehideout::v1::*;

pub struct AdminSvc {
    #[allow(dead_code)]
    state: AppState,
}

impl AdminSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl AdminService for AdminSvc {
    async fn get_overview(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, GetOverviewRequest>,
    ) -> ServiceResult<GetOverviewResponse> {
        Err(ConnectError::unimplemented("GetOverview is not implemented yet"))
    }

    async fn get_activity(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, GetActivityRequest>,
    ) -> ServiceResult<GetActivityResponse> {
        Err(ConnectError::unimplemented("GetActivity is not implemented yet"))
    }

    async fn list_guilds(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListGuildsRequest>,
    ) -> ServiceResult<ListGuildsResponse> {
        Err(ConnectError::unimplemented("ListGuilds is not implemented yet"))
    }

    async fn list_users(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListUsersRequest>,
    ) -> ServiceResult<ListUsersResponse> {
        Err(ConnectError::unimplemented("ListUsers is not implemented yet"))
    }

    async fn set_admin(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, SetAdminRequest>,
    ) -> ServiceResult<SetAdminResponse> {
        Err(ConnectError::unimplemented("SetAdmin is not implemented yet"))
    }

    async fn list_files(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListFilesRequest>,
    ) -> ServiceResult<ListFilesResponse> {
        Err(ConnectError::unimplemented("ListFiles is not implemented yet"))
    }

    async fn delete_file(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, DeleteFileRequest>,
    ) -> ServiceResult<DeleteFileResponse> {
        Err(ConnectError::unimplemented("DeleteFile is not implemented yet"))
    }

    async fn list_audit_log(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListAuditLogRequest>,
    ) -> ServiceResult<ListAuditLogResponse> {
        Err(ConnectError::unimplemented("ListAuditLog is not implemented yet"))
    }
}

/// Placeholder implementation of the emoji service.
pub struct EmojiSvc {
    #[allow(dead_code)]
    state: AppState,
}

impl EmojiSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl EmojiService for EmojiSvc {
    async fn list_emojis(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListEmojisRequest>,
    ) -> ServiceResult<ListEmojisResponse> {
        Err(ConnectError::unimplemented("ListEmojis is not implemented yet"))
    }

    async fn create_emoji(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, CreateEmojiRequest>,
    ) -> ServiceResult<CreateEmojiResponse> {
        Err(ConnectError::unimplemented("CreateEmoji is not implemented yet"))
    }

    async fn rename_emoji(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, RenameEmojiRequest>,
    ) -> ServiceResult<RenameEmojiResponse> {
        Err(ConnectError::unimplemented("RenameEmoji is not implemented yet"))
    }

    async fn delete_emoji(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, DeleteEmojiRequest>,
    ) -> ServiceResult<DeleteEmojiResponse> {
        Err(ConnectError::unimplemented("DeleteEmoji is not implemented yet"))
    }
}
