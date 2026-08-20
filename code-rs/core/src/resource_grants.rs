use code_protocol::request_resources::ResourceGrantScope;
use code_protocol::request_resources::ResourceRequestProfile;

#[derive(Debug, Default)]
pub(crate) struct ResourceGrantState {
    next_command: Option<ResourceRequestProfile>,
    session: Option<ResourceRequestProfile>,
}

impl ResourceGrantState {
    pub(crate) fn approve(&mut self, scope: ResourceGrantScope, resources: ResourceRequestProfile) {
        if resources.is_empty() {
            return;
        }

        match scope {
            ResourceGrantScope::NextCommand => merge_profile(&mut self.next_command, resources),
            ResourceGrantScope::Session => merge_profile(&mut self.session, resources),
        }
    }

    pub(crate) fn current(
        &self,
        configured: &ResourceRequestProfile,
        outer: &ResourceRequestProfile,
    ) -> ResourceRequestProfile {
        resolve_limits(
            self.next_command.as_ref(),
            self.session.as_ref(),
            configured,
            outer,
        )
    }

    pub(crate) fn persistent_current(
        &self,
        configured: &ResourceRequestProfile,
        outer: &ResourceRequestProfile,
    ) -> ResourceRequestProfile {
        resolve_limits(None, self.session.as_ref(), configured, outer)
    }

    pub(crate) fn take_for_spawn(
        &mut self,
        configured: &ResourceRequestProfile,
        outer: &ResourceRequestProfile,
    ) -> ResourceRequestProfile {
        let next_command = self.next_command.take();
        resolve_limits(
            next_command.as_ref(),
            self.session.as_ref(),
            configured,
            outer,
        )
    }

    pub(crate) fn clamp_requested(
        resources: &ResourceRequestProfile,
        outer: &ResourceRequestProfile,
    ) -> (ResourceRequestProfile, Option<String>) {
        let effective = ResourceRequestProfile {
            memory_max_mb: clamp(resources.memory_max_mb, outer.memory_max_mb),
            pids_max: clamp(resources.pids_max, outer.pids_max),
        };
        let mut reasons = Vec::new();
        if resources.memory_max_mb != effective.memory_max_mb
            && let Some(limit) = effective.memory_max_mb
        {
            reasons.push(format!(
                "memory_max_mb clamped to {limit} by the outer host limit"
            ));
        }
        if resources.pids_max != effective.pids_max
            && let Some(limit) = effective.pids_max
        {
            reasons.push(format!(
                "pids_max clamped to {limit} by the outer host limit"
            ));
        }
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        (effective, reason)
    }
}

fn merge_profile(destination: &mut Option<ResourceRequestProfile>, source: ResourceRequestProfile) {
    let destination = destination.get_or_insert_with(ResourceRequestProfile::default);
    if let Some(source) = source.memory_max_mb {
        destination.memory_max_mb = Some(
            destination
                .memory_max_mb
                .map_or(source, |current| current.max(source)),
        );
    }
    if let Some(source) = source.pids_max {
        destination.pids_max = Some(
            destination
                .pids_max
                .map_or(source, |current| current.max(source)),
        );
    }
}

fn resolve_limits(
    next_command: Option<&ResourceRequestProfile>,
    session: Option<&ResourceRequestProfile>,
    configured: &ResourceRequestProfile,
    outer: &ResourceRequestProfile,
) -> ResourceRequestProfile {
    let memory_max_mb = next_command
        .and_then(|profile| profile.memory_max_mb)
        .or_else(|| session.and_then(|profile| profile.memory_max_mb))
        .or(configured.memory_max_mb);
    let pids_max = next_command
        .and_then(|profile| profile.pids_max)
        .or_else(|| session.and_then(|profile| profile.pids_max))
        .or(configured.pids_max);

    ResourceRequestProfile {
        memory_max_mb: clamp(memory_max_mb, outer.memory_max_mb),
        pids_max: clamp(pids_max, outer.pids_max),
    }
}

fn clamp(value: Option<u64>, outer: Option<u64>) -> Option<u64> {
    match (value, outer) {
        (Some(value), Some(outer)) => Some(value.min(outer)),
        (value, _) => value,
    }
}

pub(crate) fn configured_resource_limits() -> ResourceRequestProfile {
    #[cfg(target_os = "linux")]
    {
        ResourceRequestProfile {
            memory_max_mb: crate::cgroup::default_exec_memory_max_bytes()
                .map(|bytes| bytes / (1024 * 1024)),
            pids_max: crate::cgroup::default_exec_pids_max(),
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        ResourceRequestProfile::default()
    }
}

pub(crate) fn outer_resource_limits() -> ResourceRequestProfile {
    #[cfg(target_os = "linux")]
    {
        let limits = crate::cgroup::outer_cgroup_limits();
        ResourceRequestProfile {
            memory_max_mb: limits.memory_max_bytes.map(|bytes| bytes / (1024 * 1024)),
            pids_max: limits.pids_max,
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        ResourceRequestProfile::default()
    }
}

pub(crate) fn to_exec_cgroup_limits(
    resources: ResourceRequestProfile,
) -> crate::cgroup::ExecCgroupLimits {
    crate::cgroup::ExecCgroupLimits {
        memory_max_bytes: resources
            .memory_max_mb
            .map(|megabytes| megabytes.saturating_mul(1024 * 1024)),
        pids_max: resources.pids_max,
    }
}

pub(crate) fn memory_limit_failure_message(memory_max_bytes: u64) -> String {
    format!(
        "resource_limit_failure={{\"resource\":\"memory\",\"effective_limit\":{memory_max_bytes},\"unit\":\"bytes\"}}\nCall `request_resources` with a larger `memory_max_mb` before retrying."
    )
}

pub(crate) fn pids_limit_failure_message(pids_max: u64) -> String {
    format!(
        "resource_limit_failure={{\"resource\":\"pids\",\"effective_limit\":{pids_max},\"unit\":\"processes\"}}\nCall `request_resources` with a larger `pids_max` before retrying."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(memory_max_mb: Option<u64>, pids_max: Option<u64>) -> ResourceRequestProfile {
        ResourceRequestProfile {
            memory_max_mb,
            pids_max,
        }
    }

    #[test]
    fn next_command_overrides_session_per_resource_and_is_consumed_once() {
        let configured = profile(Some(512), Some(256));
        let outer = ResourceRequestProfile::default();
        let mut grants = ResourceGrantState::default();
        grants.approve(ResourceGrantScope::Session, profile(None, Some(1024)));
        grants.approve(ResourceGrantScope::NextCommand, profile(Some(2048), None));

        assert_eq!(
            grants.take_for_spawn(&configured, &outer),
            profile(Some(2048), Some(1024))
        );
        assert_eq!(
            grants.take_for_spawn(&configured, &outer),
            profile(Some(512), Some(1024))
        );
    }

    #[test]
    fn resource_limit_grant_merges_partial_session_approvals() {
        let configured = profile(Some(512), Some(256));
        let outer = ResourceRequestProfile::default();
        let mut grants = ResourceGrantState::default();
        grants.approve(ResourceGrantScope::Session, profile(Some(2048), Some(1024)));
        grants.approve(ResourceGrantScope::Session, profile(None, Some(1536)));

        assert_eq!(
            grants.current(&configured, &outer),
            profile(Some(2048), Some(1536))
        );
    }

    #[test]
    fn resource_limit_grant_merges_partial_next_command_approvals() {
        let configured = profile(Some(512), Some(256));
        let outer = ResourceRequestProfile::default();
        let mut grants = ResourceGrantState::default();
        grants.approve(ResourceGrantScope::NextCommand, profile(Some(2048), None));
        grants.approve(ResourceGrantScope::NextCommand, profile(None, Some(768)));

        assert_eq!(
            grants.take_for_spawn(&configured, &outer),
            profile(Some(2048), Some(768))
        );
        assert_eq!(grants.take_for_spawn(&configured, &outer), configured);
    }

    #[test]
    fn persistent_limits_ignore_pending_next_command_grants() {
        let configured = profile(Some(512), Some(256));
        let outer = ResourceRequestProfile::default();
        let mut grants = ResourceGrantState::default();
        grants.approve(
            ResourceGrantScope::NextCommand,
            profile(Some(2048), Some(768)),
        );

        assert_eq!(grants.persistent_current(&configured, &outer), configured);
        assert_eq!(
            grants.current(&configured, &outer),
            profile(Some(2048), Some(768))
        );
    }

    #[test]
    fn smaller_follow_up_approval_does_not_reduce_existing_grant() {
        let configured = profile(Some(512), Some(256));
        let outer = ResourceRequestProfile::default();
        let mut grants = ResourceGrantState::default();
        grants.approve(
            ResourceGrantScope::NextCommand,
            profile(Some(2048), Some(768)),
        );
        grants.approve(
            ResourceGrantScope::NextCommand,
            profile(Some(1024), Some(512)),
        );

        assert_eq!(
            grants.take_for_spawn(&configured, &outer),
            profile(Some(2048), Some(768))
        );
    }

    #[test]
    fn smaller_session_approval_does_not_reduce_existing_grant() {
        let configured = profile(Some(512), Some(256));
        let outer = ResourceRequestProfile::default();
        let mut grants = ResourceGrantState::default();
        grants.approve(ResourceGrantScope::Session, profile(Some(2048), Some(768)));
        grants.approve(ResourceGrantScope::Session, profile(Some(1024), Some(512)));

        assert_eq!(
            grants.persistent_current(&configured, &outer),
            profile(Some(2048), Some(768))
        );
    }

    #[test]
    fn outer_limits_clamp_approved_values() {
        let configured = profile(Some(512), Some(256));
        let outer = profile(Some(1536), Some(600));
        let mut grants = ResourceGrantState::default();
        grants.approve(ResourceGrantScope::Session, profile(Some(4096), Some(2048)));

        assert_eq!(
            grants.current(&configured, &outer),
            profile(Some(1536), Some(600))
        );
    }

    #[test]
    fn clamping_reports_requested_and_effective_differences() {
        let requested = profile(Some(4096), Some(2048));
        let outer = profile(Some(1536), Some(600));

        let (effective, reason) = ResourceGrantState::clamp_requested(&requested, &outer);

        assert_eq!(effective, profile(Some(1536), Some(600)));
        assert_eq!(
            reason.as_deref(),
            Some(
                "memory_max_mb clamped to 1536 by the outer host limit; pids_max clamped to 600 by the outer host limit"
            )
        );
    }

    #[test]
    fn empty_response_does_not_change_grants() {
        let configured = profile(Some(512), Some(256));
        let outer = ResourceRequestProfile::default();
        let mut grants = ResourceGrantState::default();
        grants.approve(ResourceGrantScope::Session, profile(Some(2048), None));
        grants.approve(
            ResourceGrantScope::NextCommand,
            ResourceRequestProfile::default(),
        );

        assert_eq!(
            grants.current(&configured, &outer),
            profile(Some(2048), Some(256))
        );
    }

    #[test]
    fn failure_guidance_is_machine_readable_and_actionable() {
        assert_eq!(
            memory_limit_failure_message(1_073_741_824),
            "resource_limit_failure={\"resource\":\"memory\",\"effective_limit\":1073741824,\"unit\":\"bytes\"}\nCall `request_resources` with a larger `memory_max_mb` before retrying."
        );
        assert_eq!(
            pids_limit_failure_message(512),
            "resource_limit_failure={\"resource\":\"pids\",\"effective_limit\":512,\"unit\":\"processes\"}\nCall `request_resources` with a larger `pids_max` before retrying."
        );
    }
}
