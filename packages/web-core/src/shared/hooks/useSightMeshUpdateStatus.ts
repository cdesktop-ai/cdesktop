import { useQuery } from '@tanstack/react-query';

import { maintenanceApi } from '@/shared/lib/api';

export function useSightMeshUpdateStatus() {
  return useQuery({
    queryKey: ['maintenance', 'sightmesh-update'],
    queryFn: maintenanceApi.getUpdateStatus,
    refetchInterval: 2_000,
    retry: false,
  });
}
