#!/bin/bash

echo "Status: 200 OK"
echo "Content-Type: application/json"
echo ""

# Read the request body from stdin
read -t 1 REQUEST_BODY

# Parse query string parameters
QUERY_PARAMS=$(echo "$QUERY_STRING" | tr '&' '\n')

echo "{"
echo "  \"message\": \"CGI POST Handler\","
echo "  \"method\": \"$REQUEST_METHOD\","
echo "  \"path\": \"$SCRIPT_NAME\","
echo "  \"query_string\": \"$QUERY_STRING\","
echo "  \"content_length\": $CONTENT_LENGTH,"
echo "  \"content_type\": \"$CONTENT_TYPE\","
echo "  \"request_body\": \"$REQUEST_BODY\","
echo "  \"environment\": {"
echo "    \"remote_addr\": \"$REMOTE_ADDR\","
echo "    \"server_name\": \"$SERVER_NAME\","
echo "    \"server_port\": \"$SERVER_PORT\","
echo "    \"http_user_agent\": \"$HTTP_USER_AGENT\","
echo "    \"http_host\": \"$HTTP_HOST\""
echo "  }"
echo "}"
