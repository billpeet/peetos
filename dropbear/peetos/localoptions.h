#ifndef PEETOS_DROPBEAR_LOCALOPTIONS_H
#define PEETOS_DROPBEAR_LOCALOPTIONS_H

/*
 * PeetOS dropbear configuration.
 * Keep features minimal until PTY and multiuser support are fully implemented.
 */

/* Server authentication */
#define DROPBEAR_SVR_PASSWORD_AUTH 0
#define DROPBEAR_SVR_PAM_AUTH 0
#define DROPBEAR_SVR_PUBKEY_AUTH 1
#define DROPBEAR_SVR_PUBKEY_OPTIONS 1

/* Client authentication */
#define DROPBEAR_CLI_PASSWORD_AUTH 0
#define DROPBEAR_CLI_PUBKEY_AUTH 1
#define DROPBEAR_USE_PASSWORD_ENV 0

/* Disable agent forwarding (requires UNIX domain sockets). */
#define DROPBEAR_SVR_AGENTFWD 0
#define DROPBEAR_CLI_AGENTFWD 0

/* Disable forwarding features for now. */
#define DROPBEAR_SVR_LOCALTCPFWD 0
#define DROPBEAR_SVR_REMOTETCPFWD 0
#define DROPBEAR_SVR_LOCALSTREAMFWD 0
#define DROPBEAR_CLI_LOCALTCPFWD 0
#define DROPBEAR_CLI_REMOTETCPFWD 0
#define DROPBEAR_CLI_PROXYCMD 0
#define DROPBEAR_CLI_NETCAT 0

/* Disable MOTD lookup. */
#define DO_MOTD 0

#endif /* PEETOS_DROPBEAR_LOCALOPTIONS_H */
