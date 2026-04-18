#!/bin/bash
set -e

echo "Setting up autovocoder development environment..."

#######################
# DNS Configuration
#######################
export DNS_SERVER="${DNS_SERVER:-$(getent hosts pfsense1.basedweights.com 2>/dev/null | awk '{print $1}' | head -1 || echo '8.8.8.8')}"
echo "export DNS_SERVER=$DNS_SERVER" >>~/.bashrc
echo "Using DNS server: $DNS_SERVER"

#######################
# Docker Buildx
#######################
mkdir -p /proc/sys/fs/binfmt_misc

if docker ps &> /dev/null; then
  docker run --rm --privileged tonistiigi/binfmt --install all

  if docker buildx ls | grep -q "project-builder"; then
    echo "Removing existing project-builder builder..."
    docker buildx rm project-builder || true
  fi

  docker buildx create --name project-builder \
    --driver docker-container \
    --driver-opt network=host \
    --driver-opt env.BUILDKITD_FLAGS="--dns=$DNS_SERVER" \
    --use
  docker buildx inspect --bootstrap
else
  echo "Docker daemon is not available, skipping Docker Buildx setup"
fi

#######################
# Setup local secrets if provided
#######################
if [ -n "$DOCKER_LOGIN_B64" ]; then
  echo "DOCKER_LOGIN_B64 is set, setting up local secrets"
  mkdir -p ~/.docker
  echo "$DOCKER_LOGIN_B64" | base64 -d >~/.docker/config.json
else
  echo "DOCKER_LOGIN_B64 is not set, using empty config"
fi

if [ -n "$GITLAB_TOKEN_B64" ]; then
  echo "GITLAB_TOKEN_B64 is set, setting up local secrets"
  # shellcheck disable=SC2155
  export GITLAB_TOKEN="$(echo "$GITLAB_TOKEN_B64" | base64 -d)"
  echo "export GITLAB_TOKEN=$GITLAB_TOKEN" >>~/.bashrc
else
  echo "GITLAB_TOKEN_B64 is not set, using empty token"
fi

if [ -n "$GIT_CREDENTIALS_B64" ]; then
  echo "GIT_CREDENTIALS_B64 is set, setting up local secrets"
  echo "$GIT_CREDENTIALS_B64" | base64 -d >~/.git-credentials
else
  echo "GIT_CREDENTIALS_B64 is not set, using empty credentials"
fi

#######################
# kubectl / helm completion
#######################
if command -v kubectl &> /dev/null; then
  echo "source <(kubectl completion bash)" >> ~/.bashrc
  echo "alias k=kubectl" >> ~/.bashrc
  echo "complete -F __start_kubectl k" >> ~/.bashrc
fi

if command -v helm &> /dev/null; then
  helm completion bash | sudo tee /etc/bash_completion.d/helm > /dev/null
  echo "source <(helm completion bash)" >> ~/.bashrc
fi

#######################
# Git
#######################
git config --global --add safe.directory "$(pwd)"

#######################
# User post-create hook
#######################
DEVCONTAINER_USER_POST_SCRIPT_FILE=".devcontainer/.user-post-create.sh"
if [ -f "${DEVCONTAINER_USER_POST_SCRIPT_FILE}" ]; then
  bash ${DEVCONTAINER_USER_POST_SCRIPT_FILE}
else
  echo "${DEVCONTAINER_USER_POST_SCRIPT_FILE} not found, skipping"
  touch ${DEVCONTAINER_USER_POST_SCRIPT_FILE}
  {
    echo "#!/bin/bash"
    echo "# Add any user specific post create commands here"
  } >>${DEVCONTAINER_USER_POST_SCRIPT_FILE}
fi

echo "Post-create complete!"
