// The client-side fake: a ServiceWorker that answers the same endpoints as `serve.mjs`,
// from the same shared demo data, so the page runs on any static host with no backend.
// index.html registers this worker only when the page is served as plain static files;
// `serve.mjs` serves its own fake (or a proxy) and rewrites the marker that keeps this off.
//
// The worker sits at the origin root with the default scope: the core grammar is
// origin-absolute (`/repository/<name>/...`, `/service/rest/v1/search/assets`),
// so only a deployment at the origin root puts the whole mount inside the scope.
// Names are `[A-Za-z0-9._-]`, so a mount root below can occur in a pathname
// only where it is meant to.

import { fakeNexus } from './fake-nexus.mjs'

const SEARCH = '/service/rest/v1/search/assets'
const REPOSITORY = '/repository/'

self.addEventListener('install', () => self.skipWaiting())
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()))

self.addEventListener('fetch', (event) => {
  if (event.request.method !== 'GET') return
  const url = new URL(event.request.url)
  // The page may be reached under a deployment prefix: strip it by handing the fake
  // the path from the mount root on. The search root wins when both appear, because
  // it is the deeper, exact endpoint.
  const searchAt = mountAt(url.pathname, SEARCH)
  const repositoryAt = mountAt(url.pathname, REPOSITORY)
  const at = searchAt >= 0 && (repositoryAt < 0 || searchAt > repositoryAt) ? searchAt : repositoryAt
  if (at < 0) return
  const fake = fakeNexus(url.pathname.slice(at), url.searchParams)
  event.respondWith(new Response(fake.body, {
    status: fake.status,
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  }))
})

// Where a mount root starts such that it begins the pathname or follows a `/`.
// A file named `service` or `repository` deeper in the tree must not match.
function mountAt(pathname, root) {
  let at = pathname.lastIndexOf(root)
  while (at > 0 && pathname[at - 1] !== '/') at = pathname.lastIndexOf(root, at - 1)
  return at
}
