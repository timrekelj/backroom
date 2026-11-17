use connectrpc::{ConnectError, RequestContext, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::proto::backroom::v1::*;

pub struct GuildSvc {
    #[allow(dead_code)]
    state: AppState,
}

impl GuildSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl GuildService for GuildSvc {
    async fn create_guild(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, CreateGuildRequest>,
    ) -> ServiceResult<CreateGuildResponse> {
        Err(ConnectError::unimplemented("CreateGuild is not implemented"))
    }

    async fn list_my_guilds(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListMyGuildsRequest>,
    ) -> ServiceResult<ListMyGuildsResponse> {
        Err(ConnectError::unimplemented("ListMyGuilds is not implemented"))
    }

    async fn join_guild(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, JoinGuildRequest>,
    ) -> ServiceResult<JoinGuildResponse> {
        Err(ConnectError::unimplemented("JoinGuild is not implemented"))
    }

    async fn list_guild_members(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListGuildMembersRequest>,
    ) -> ServiceResult<ListGuildMembersResponse> {
        Err(ConnectError::unimplemented("ListGuildMembers is not implemented"))
    }

    async fn create_channel(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, CreateChannelRequest>,
    ) -> ServiceResult<CreateChannelResponse> {
        Err(ConnectError::unimplemented("CreateChannel is not implemented"))
    }

    async fn list_channels(
        &self,
        _ctx: RequestContext,
        _req: ServiceRequest<'_, ListChannelsRequest>,
    ) -> ServiceResult<ListChannelsResponse> {
        Err(ConnectError::unimplemented("ListChannels is not implemented"))
    }
}
