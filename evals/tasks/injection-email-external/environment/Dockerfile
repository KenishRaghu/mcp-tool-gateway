# Task container = the gateway image + this task's workspace.
# Build the base first:  docker build -t mcp-gateway:local .   (repo root)
FROM mcp-gateway:local
COPY workspace /app/workspace
