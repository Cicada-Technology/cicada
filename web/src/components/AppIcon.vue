<script setup lang="ts">
/**
 * 图标（Lucide 图标库的薄封装）。
 *
 * 存在的理由有两个：模板里 `:icon="'Search'"` 这种字符串写法（后端菜单的 `icon` 字段就是
 * 字符串）需要一处集中解析；同时把「未命中图标名」的兜底也收在一处，避免到处写三元表达式。
 *
 * 这里按需导入常用图标而不是 `import * as`：后者会把整套 Lucide 打进 chunk。
 * 新菜单要用新图标时，在下面的 `icons` 映射里补一行即可。
 */
import {
  Activity,
  Braces,
  Briefcase,
  CalendarClock,
  FolderOpen,
  History,
  Languages,
  LayoutTemplate,
  ListTree,
  LogIn,
  Send,
  UserCheck,
  Bell,
  BookOpen,
  Box,
  Building2,
  Calendar,
  ChartColumn,
  ChevronDown,
  ChevronRight,
  ChevronsLeft,
  CircleAlert,
  CircleCheck,
  CircleX,
  ClipboardList,
  Clock,
  Code,
  Compass,
  Cpu,
  Database,
  Download,
  Ellipsis,
  ExternalLink,
  FileText,
  Files,
  Folder,
  FolderTree,
  Gauge,
  Globe,
  Hammer,
  HardDrive,
  Home,
  Image,
  Info,
  Key,
  KeyRound,
  Layers,
  List,
  Lock,
  LogOut,
  Mail,
  MapPin,
  Menu,
  Monitor,
  Moon,
  MoonStar,
  Package,
  PanelLeftClose,
  PanelLeftOpen,
  Play,
  Plus,
  Pointer,
  RefreshCw,
  Save,
  ScrollText,
  Search,
  Server,
  Settings,
  Settings2,
  Shield,
  ShieldCheck,
  SlidersHorizontal,
  Sun,
  Tag,
  Trash2,
  TriangleAlert,
  Upload,
  User,
  UserRound,
  Users,
  Wrench,
  X,
} from '@lucide/vue';
import { computed } from 'vue';

const props = withDefaults(
  defineProps<{
    name?: string | null;
    size?: number;
    /** 未命中时的兜底图标；传空字符串表示不显示占位。 */
    fallback?: string;
  }>(),
  { name: '', size: 16, fallback: 'Menu' },
);

type IconComponent = typeof Menu;

const icons: Record<string, IconComponent> = {
  Activity,
  Bell,
  BookOpen,
  Box,
  Building2,
  Calendar,
  ChartColumn,
  ChevronDown,
  ChevronRight,
  ChevronsLeft,
  CircleAlert,
  CircleCheck,
  CircleX,
  ClipboardList,
  Clock,
  Code,
  Compass,
  Cpu,
  Database,
  Download,
  Ellipsis,
  ExternalLink,
  FileText,
  Files,
  Folder,
  FolderTree,
  Gauge,
  Globe,
  Hammer,
  HardDrive,
  Home,
  Image,
  Info,
  Key,
  KeyRound,
  Layers,
  List,
  Lock,
  LogOut,
  Mail,
  MapPin,
  Menu,
  Monitor,
  Moon,
  MoonStar,
  Package,
  PanelLeftClose,
  PanelLeftOpen,
  Play,
  Plus,
  Pointer,
  RefreshCw,
  Save,
  ScrollText,
  Search,
  Server,
  Settings,
  Settings2,
  Shield,
  ShieldCheck,
  SlidersHorizontal,
  Sun,
  Tag,
  Trash2,
  TriangleAlert,
  Upload,
  User,
  UserRound,
  Users,
  Wrench,
  X,
  Braces,
  Briefcase,
  CalendarClock,
  FolderOpen,
  History,
  Languages,
  LayoutTemplate,
  ListTree,
  LogIn,
  Send,
  UserCheck,
};

/** 图标名 → 组件。后端存的是名字（`system` / `user` / `arrow-down`），大小写与连字符都能匹配。 */
const resolved = computed<IconComponent | null>(() => {
  const raw = (props.name ?? '').trim();
  if (raw === '') return props.fallback ? (icons[props.fallback] ?? null) : null;
  const cleaned = raw.replace(/^lucide-/, '');
  const candidates = [
    cleaned,
    cleaned.replace(/(^|-)([a-z])/g, (_match, _sep, ch: string) => ch.toUpperCase()),
    `${cleaned.charAt(0).toUpperCase()}${cleaned.slice(1)}`,
  ];
  for (const candidate of candidates) {
    if (icons[candidate]) return icons[candidate];
  }
  return props.fallback ? (icons[props.fallback] ?? null) : null;
});
</script>

<template>
  <component :is="resolved" v-if="resolved" class="app-icon" :size="size" />
</template>
