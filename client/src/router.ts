import { createRouter, createWebHistory } from 'vue-router'
import LoginView from './views/LoginView.vue'
import PagesListView from './views/PagesListView.vue'
import PageEditView from './views/PageEditView.vue'
import TagsView from './views/TagsView.vue'
import FilesView from './views/FilesView.vue'
import FileEditView from './views/FileEditView.vue'
import GalleriesView from './views/GalleriesView.vue'
import GalleryEditView from './views/GalleryEditView.vue'
import MenuView from './views/MenuView.vue'
import DesignView from './views/DesignView.vue'
import TokensView from './views/TokensView.vue'
import UsersView from './views/UsersView.vue'
import AssistantView from './views/AssistantView.vue'
import McpServersView from './views/McpServersView.vue'
import ProvidersView from './views/ProvidersView.vue'
import ModelsView from './views/ModelsView.vue'
import ToolPermissionsView from './views/ToolPermissionsView.vue'
import { useAuthStore } from './stores/auth'

const routes = [
  { path: '/login', component: LoginView, meta: { public: true } },
  { path: '/', redirect: '/pages' },
  { path: '/pages', component: PagesListView },
  { path: '/pages/new', component: PageEditView, props: { create: true } },
  { path: '/pages/:id/edit', component: PageEditView, props: true },
  { path: '/tags', component: TagsView },
  { path: '/files', component: FilesView },
  { path: '/files/:id/edit', component: FileEditView, props: true },
  { path: '/galleries', component: GalleriesView },
  { path: '/galleries/new', component: GalleryEditView, props: { create: true } },
  { path: '/galleries/:id/edit', component: GalleryEditView, props: true },
  { path: '/menu', component: MenuView },
  { path: '/menu/:id/edit', component: MenuView, props: true },
  { path: '/design', component: DesignView },
  { path: '/tokens', component: TokensView },
  { path: '/users', component: UsersView },
  { path: '/assistant', component: AssistantView },
  { path: '/mcp-servers', component: McpServersView },
  { path: '/providers', component: ProvidersView },
  { path: '/models', component: ModelsView },
  { path: '/tool-permissions', component: ToolPermissionsView },
]

export const router = createRouter({
  history: createWebHistory('/admin/'),
  routes,
})

router.beforeEach(async (to) => {
  const auth = useAuthStore()
  if (!auth.checked) {
    await auth.checkSession()
  }
  if (!to.meta.public && !auth.isLoggedIn) {
    return '/login'
  }
  if (to.path === '/login' && auth.isLoggedIn) {
    return '/pages'
  }
})
