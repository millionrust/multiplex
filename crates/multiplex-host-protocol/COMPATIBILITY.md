# Host protocol compatibility policy

The committed `proto/host.proto` schema is the source of truth. Wire tags and enum
numbers are permanent. Removed fields are reserved by number and name. Existing
field types, cardinality, oneof membership, and semantic meaning are not changed.

Minor versions may add optional fields, enum values, capabilities, and noncritical
messages. A peer negotiates the highest common minor version and only advertises
implemented capabilities. Code that must proxy a future minor payload keeps its
original protobuf bytes; decoding into generated Rust values is not a lossless
proxy operation.

A major version change is incompatible. The connection may return only a bounded
`PROTOCOL_INCOMPATIBLE` error containing supported ranges and must not apply a
mutation. Golden binary and JSON manifests are reviewed with every schema change.
