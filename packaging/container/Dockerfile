# The nxr image: the static musl binary out of the release archive on a base
# that carries a trust store.
#
# The binary is static, so a base without a libc works, but TLS verification
# reads the system trust store through rustls-native-certs: a scratch base has
# no /etc/ssl/certs/ca-certificates.crt, and every HTTPS request to Nexus fails
# there with an unknown-issuer error. distroless/static carries exactly that
# bundle plus a nonroot user, and nothing else: no shell, no package manager.
#
# The build context is staged, never the checkout: bin/<arch>/nxr, where <arch>
# is the Docker platform name (amd64, arm64). The release workflow unpacks the
# archives it already built into that layout, so the binary that ships is the
# binary the build job smoked, and no step runs under emulation.

FROM gcr.io/distroless/static:nonroot

# The automatic platform argument: BuildKit sets it per platform, and the COPY
# below reads the matching binary. A build that does not set it fails loudly
# with a missing path instead of shipping the wrong architecture.
ARG TARGETARCH

# OCI labels: `source` is what links the GHCR package to this repository, the
# release workflow passes the version and the commit.
ARG VERSION=0.0.0
ARG REVISION=unknown
LABEL org.opencontainers.image.title="nxr" \
      org.opencontainers.image.description="curl for a Sonatype Nexus raw repository" \
      org.opencontainers.image.source="https://github.com/Lipen/nexus-raw" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}"

# The one build step: a copy, no RUN. An image of nothing but copied files
# builds for any platform without emulation, and the mode travels with the file
# (the image has no shell to chmod it later).
COPY --chown=nonroot:nonroot bin/${TARGETARCH}/nxr /nxr

# argv passes straight through: `docker run --rm <image> --version` is
# `/nxr --version` inside.
ENTRYPOINT ["/nxr"]
