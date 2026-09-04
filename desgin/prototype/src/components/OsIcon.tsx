import React from 'react';
import {
  Apple,
  LayoutGrid,
  Mountain,
  CircleDot,
  Disc,
  Box,
  Triangle,
  Flame,
  Terminal,
  LucideIcon,
} from 'lucide-react';
import { OsType } from '../types';

export interface OsConfig {
  label: string;
  shortName: string;
  icon: LucideIcon;
  color: string;
  textColor: string;
  bgColor: string;
  borderColor: string;
}

export const OS_CONFIG_MAP: Record<string, OsConfig> = {
  ubuntu: {
    label: 'Ubuntu Linux',
    shortName: 'Ubuntu',
    icon: CircleDot,
    color: '#E95420',
    textColor: 'text-[#E95420]',
    bgColor: 'bg-[#E95420]/15',
    borderColor: 'border-[#E95420]/40',
  },
  debian: {
    label: 'Debian GNU/Linux',
    shortName: 'Debian',
    icon: Disc,
    color: '#D70A53',
    textColor: 'text-[#E11D48]',
    bgColor: 'bg-[#D70A53]/15',
    borderColor: 'border-[#D70A53]/40',
  },
  centos: {
    label: 'CentOS / RHEL',
    shortName: 'CentOS',
    icon: Box,
    color: '#9333EA',
    textColor: 'text-[#A855F7]',
    bgColor: 'bg-[#9333EA]/15',
    borderColor: 'border-[#9333EA]/40',
  },
  alpine: {
    label: 'Alpine Linux',
    shortName: 'Alpine',
    icon: Mountain,
    color: '#0EA5E9',
    textColor: 'text-[#38BDF8]',
    bgColor: 'bg-[#0EA5E9]/15',
    borderColor: 'border-[#0EA5E9]/40',
  },
  macos: {
    label: 'macOS Darwin',
    shortName: 'macOS',
    icon: Apple,
    color: '#E5E7EB',
    textColor: 'text-zinc-200',
    bgColor: 'bg-zinc-700/30',
    borderColor: 'border-zinc-500/40',
  },
  windows: {
    label: 'Windows Server',
    shortName: 'Windows',
    icon: LayoutGrid,
    color: '#0078D4',
    textColor: 'text-[#38BDF8]',
    bgColor: 'bg-[#0078D4]/15',
    borderColor: 'border-[#0078D4]/40',
  },
  arch: {
    label: 'Arch Linux',
    shortName: 'Arch',
    icon: Triangle,
    color: '#1793D1',
    textColor: 'text-[#38BDF8]',
    bgColor: 'bg-[#1793D1]/15',
    borderColor: 'border-[#1793D1]/40',
  },
  freebsd: {
    label: 'FreeBSD / BSD',
    shortName: 'FreeBSD',
    icon: Flame,
    color: '#EF4444',
    textColor: 'text-[#F87171]',
    bgColor: 'bg-[#EF4444]/15',
    borderColor: 'border-[#EF4444]/40',
  },
};

export function getOsConfig(osType?: string | OsType): OsConfig {
  const normalized = (osType || '').toLowerCase().trim();
  if (OS_CONFIG_MAP[normalized]) {
    return OS_CONFIG_MAP[normalized];
  }
  // Check substrings
  if (normalized.includes('ubuntu')) return OS_CONFIG_MAP.ubuntu;
  if (normalized.includes('debian')) return OS_CONFIG_MAP.debian;
  if (normalized.includes('cent') || normalized.includes('rhel') || normalized.includes('redhat') || normalized.includes('rocky')) return OS_CONFIG_MAP.centos;
  if (normalized.includes('alpin')) return OS_CONFIG_MAP.alpine;
  if (normalized.includes('mac') || normalized.includes('darwin') || normalized.includes('apple')) return OS_CONFIG_MAP.macos;
  if (normalized.includes('win')) return OS_CONFIG_MAP.windows;
  if (normalized.includes('arch')) return OS_CONFIG_MAP.arch;
  if (normalized.includes('bsd')) return OS_CONFIG_MAP.freebsd;

  // Fallback default Linux
  return {
    label: 'Linux OS',
    shortName: osType ? osType.toUpperCase() : 'Linux',
    icon: Terminal,
    color: '#10B981',
    textColor: 'text-[#34D399]',
    bgColor: 'bg-[#10B981]/15',
    borderColor: 'border-[#10B981]/40',
  };
}

interface OsIconProps {
  osType?: string | OsType;
  className?: string;
  size?: number;
}

export const OsIcon: React.FC<OsIconProps> = ({ osType, className = 'w-3.5 h-3.5', size }) => {
  const config = getOsConfig(osType);
  const IconComponent = config.icon;
  return (
    <IconComponent
      size={size}
      className={`${className} ${config.textColor}`}
      aria-label={config.label}
    />
  );
};

interface OsBadgeProps {
  osType?: string | OsType;
  className?: string;
  iconClassName?: string;
  showName?: boolean;
  size?: 'sm' | 'md';
}

export const OsBadge: React.FC<OsBadgeProps> = ({
  osType,
  className = '',
  iconClassName = 'w-3 h-3',
  showName = true,
  size = 'sm',
}) => {
  const config = getOsConfig(osType);
  const IconComponent = config.icon;

  return (
    <span
      title={`Operating System: ${config.label}`}
      className={`inline-flex items-center gap-1 rounded font-mono font-medium border transition-colors ${
        size === 'sm' ? 'text-[9.5px] px-1.5 py-0.5' : 'text-xs px-2 py-0.5'
      } ${config.bgColor} ${config.borderColor} ${config.textColor} ${className}`}
    >
      <IconComponent className={`${iconClassName} shrink-0`} />
      {showName && <span>{config.shortName}</span>}
    </span>
  );
};
