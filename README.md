# docker-caddy-rs

Output of `--help`:

```
Watch docker for Container events, write those out to a set of Caddy snippets, then trigger a reload of both Caddy instances

Usage: docker-caddyfile-updater [OPTIONS] --local-caddy-snippets-dir <LOCAL_CADDY_SNIPPETS_DIR> --docker-caddy-snippets-dir <DOCKER_CADDY_SNIPPETS_DIR> --label-prefix <LABEL_PREFIX> --local-domain-prefix <LOCAL_DOMAIN_PREFIX> --domain-name <DOMAIN_NAME>

Options:
      --local-caddy-bin-path <LOCAL_CADDY_BIN_PATH>            Path to the "local" Caddy binary, which handles SSL termination and proxies to the Docker Caddy instance [env:
                                                               LOCAL_CADDY_BIN_PATH=] [default: /usr/local/bin/caddy] [aliases: lcbp]
      --local-caddy-config-dir <LOCAL_CADDY_CONFIG_DIR>        Path to the "local" Caddy configuration directory, used to set the working directory when reloading Caddy [env:
                                                               LOCAL_CADDY_CONFIG_DIR=] [default: /usr/local/etc] [aliases: lccd]
      --local-caddy-snippets-dir <LOCAL_CADDY_SNIPPETS_DIR>    Directory to write the "local" snippets out to (Caddy will then import these) [env: LOCAL_CADDY_SNIPPETS_DIR=] [aliases:
                                                               lcsd]
      --local-caddy-on-docker                                  Is the "local" Caddy actually running on docker rather than the host? Could be the case if the "local" Caddy is using Host
                                                               networking, for example [env: LOCAL_CADDY_ON_DOCKER=] [aliases: lcod]
      --docker-caddy-bin-path <DOCKER_CADDY_BIN_PATH>          Path to the Caddy binary inside the Docker file (defaults to just "caddy" as it's on the path) [env: DOCKER_CADDY_BIN_PATH=]
                                                               [default: caddy] [aliases: dcbp]
      --docker-caddy-config-dir <DOCKER_CADDY_CONFIG_DIR>      Path of the Caddy configuration directory inside Docker. Only used to set the working directory when reloading Caddy [env:
                                                               DOCKER_CADDY_CONFIG_DIR=] [default: /etc/caddy] [aliases: dccd]
      --docker-caddy-snippets-dir <DOCKER_CADDY_SNIPPETS_DIR>  Directory to write the snippets for the second Caddy instance. This should be a directory that is on the host machine and is
                                                               mounted into Docker [env: DOCKER_CADDY_SNIPPETS_DIR=] [aliases: dcsd]
      --label-prefix <LABEL_PREFIX>                            The prefix for the labels used to determine what should and should not be exposed via Caddy. e.g., "my.name" Available
                                                               labels are: * app - the name of the application, prepended to the domain or local domain * port - the port the app runs on
                                                               (mandatory, no default) * external - if the app will be exposed via the domain_name (true), or the local domain (otherwise)
                                                               * auth (oidc, headers, none) - if headers, include the "auth-headers" snippet, otherwise do nothing [env: LABEL_PREFIX=]
                                                               [aliases: lp]
      --local-domain-prefix <LOCAL_DOMAIN_PREFIX>              Prefix for the local domain, used by the generated Caddy snippets for anything where "external" is false or absent [env:
                                                               LOCAL_DOMAIN_PREFIX=] [aliases: ldp]
      --domain-name <DOMAIN_NAME>                              The general domain name, e.g., example.com [env: DOMAIN_NAME=] [aliases: dn]
      --docker-bin-path <DOCKER_BIN_PATH>                      The path to the docker executable on the host system, used to shell out and trigger a reload of Caddy (should be replaced by
                                                               using the Docker API) [env: DOCKER_BIN_PATH=] [default: /usr/local/bin/docker] [aliases: dbp]
      --docker-socket-path <DOCKER_SOCKET_PATH>                Path to the docker.sock file, used to communicate with the Docker API [env: DOCKER_SOCKET_PATH=] [default:
                                                               /var/run/docker.sock] [aliases: dsp]
  -h, --help                                                   Print help (see more with '--help')
```
