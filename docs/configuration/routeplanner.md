---
description: Route planner endpoint behavior in RavaLink.
---

# Route planner

The Lavalink v4 route planner API is present in RavaLink for protocol compatibility, but a native route planner is not implemented yet.

Current behavior:

- GET /v4/routeplanner/status returns HTTP 204 to indicate that no route planner is active.
- POST /v4/routeplanner/free/address returns HTTP 500 because there is no active planner to modify.
- POST /v4/routeplanner/free/all returns HTTP 500 for the same reason.

RavaLink does not currently support rotating outbound source-provider IP addresses through route-planner strategies. Do not configure a deployment on the assumption that route rotation is available.

Implementation status is tracked in the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md).
