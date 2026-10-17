#!/bin/bash
set -e
mkdir -p /app/data
# Seta a URL do banco para o volume persistente
export DATABASE_URL="sqlite:/app/data/monitor.db?mode=rwc"
# Tenta popular se o banco estiver vazio ou desatualizado (silencioso se falhar)
monitor-diarios scan --since $(date -d '30 days ago' +%Y-%m-%d) || echo 'Aviso: scan falhou, servindo dados existentes'
# Inicia o servidor
exec monitor-diarios serve --addr 0.0.0.0:8080
