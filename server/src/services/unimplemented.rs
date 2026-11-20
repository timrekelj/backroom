//! Placeholder implementations of the media and admin services; every
//! method returns `unimplemented` until the feature is built.

use connectrpc::{ConnectError, RequestContext, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::proto::thehideout::v1::*;

pub struct MediaSvc {
    #[allow(dead_code)]
    state: AppState,
}

impl MediaSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl MediaService for MediaSvc {
    async fn upload_avatar(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, UploadAvatarRequest>,
    ) -> ServiceResult<UploadAvatarResponse> {
        Err(ConnectError::unimplemented("UploadAvatar is not implemented yet"))
    }

    async fn remove_avatar(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, RemoveAvatarRequest>,
    ) -> ServiceResult<RemoveAvatarResponse> {
        Err(ConnectError::unimplemented("RemoveAvatar is not implemented yet"))
    }

    async fn upload_guild_icon(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, UploadGuildIconRequest>,
    ) -> ServiceResult<UploadGuildIconResponse> {
        Err(ConnectError::unimplemented("UploadGuildIcon is not implemented yet"))
    }

    async fn remove_guild_icon(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, RemoveGuildIconRequest>,
    ) -> ServiceResult<RemoveGuildIconResponse> {
        Err(ConnectError::unimplemented("RemoveGuildIcon is not implemented yet"))
    }
}

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
}
