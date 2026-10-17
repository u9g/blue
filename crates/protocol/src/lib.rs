//! Generated types for every Blue wire protocol. The `.proto` files in `proto/` are the source
//! of truth; this crate regenerates from them on every build.

pub mod framing;

/// Version carried in `Hello` and `Ready` handshakes. Bump when a peer can no longer
/// interoperate with the previous version.
pub const PROTOCOL_VERSION: u32 = 1;

pub mod events {
    pub mod v1 {
        tonic::include_proto!("blue.events.v1");
    }
}

pub mod bridge {
    pub mod v1 {
        tonic::include_proto!("blue.bridge.v1");
    }
}

pub mod daemon {
    pub mod v1 {
        tonic::include_proto!("blue.daemon.v1");
    }
}

pub mod client {
    pub mod v1 {
        tonic::include_proto!("blue.client.v1");
    }
}

pub mod auth {
    pub mod v1 {
        tonic::include_proto!("blue.auth.v1");
    }
}
