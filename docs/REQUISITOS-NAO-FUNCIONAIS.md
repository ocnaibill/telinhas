# Telinhas — Requisitos não funcionais (rascunho)

> Os valores abaixo são **metas provisórias** (DEC-52): servem de alvo para o protótipo e serão revisados com a medição (item 6 do roteiro).

## 1. Máquina de referência

Um notebook de uso comum com GPU integrada que tenha encoder H.264 por hardware (Intel QSV, AMD AMF ou Apple Silicon) e rede de 100 Mbps de upload (DEC-52).

## 2. Latência e qualidade

| ID | Requisito | Meta proposta |
|----|-----------|---------------|
| RNF-01 | Latência vidro a vidro (captura até exibição) com conexão P2P direta, mesma região | ≤ 300 ms |
| RNF-02 | O mesmo, via relay TURN | ≤ 500 ms |
| RNF-03 | Tempo de encode por quadro (hardware), 1080p60 | ≤ 8 ms |
| RNF-04 | FPS entregue em relação ao configurado, em condições normais de rede | ≥ 95% |
| RNF-05 | Tempo entre digitar o código e ver a sala | ≤ 2 s |
| RNF-06 | Tempo para retomar o vídeo depois de uma reconexão | ≤ 10 s |
| RNF-07 | Tempo até aparecer o primeiro quadro de uma transmissão para um espectador novo (keyframe) | ≤ 2 s |

## 3. Recursos (app)

| ID | Requisito | Meta proposta |
|----|-----------|---------------|
| RNF-08 | Memória com o app aberto, sem sala | ≤ 150 MB |
| RNF-09 | Memória em sala com 1 transmissão 1080p assistida | ≤ 400 MB |
| RNF-10 | CPU de quem transmite 1080p60 com encode por hardware | ≤ 15% de um núcleo, além do custo da própria captura |
| RNF-11 | CPU de quem assiste 1 transmissão 1080p60 com decode por hardware | ≤ 10% de um núcleo |
| RNF-12 | Janela da interface não trava ao trocar fonte ou layout | quadros da UI mantêm ≥ 60 FPS |
| RNF-13 | Tamanho do instalador | ≤ 100 MB, incluindo FFmpeg |

## 4. Rede

| ID | Requisito | Meta proposta |
|----|-----------|---------------|
| RNF-14 | A estimativa de banda de upload (RF-06) diverge do uso medido | ≤ 20% |
| RNF-15 | Perda de pacotes tolerada sem congelar a imagem | até 2% |
| RNF-16 | O app adapta o bitrate quando a banda cai, em vez de congelar | adaptação em ≤ 3 s (depende da biblioteca WebRTC, DEC-38) |

## 5. Servidor de sinalização

| ID | Requisito | Meta proposta |
|----|-----------|---------------|
| RNF-17 | Capacidade em uma máquina modesta do homelab | ≥ 200 salas e 1000 conexões simultâneas |
| RNF-18 | Memória do servidor com essa carga | ≤ 256 MB |
| RNF-19 | Latência de repasse de `signal` | ≤ 50 ms no servidor |
| RNF-20 | O servidor não grava nada em disco sobre as salas | estado só em memória; reiniciar apaga as salas |

## 6. Segurança e privacidade

| ID | Requisito |
|----|-----------|
| RNF-21 | Mídia sempre criptografada (DTLS-SRTP, padrão do WebRTC). |
| RNF-22 | Sinalização só por TLS. |
| RNF-23 | Sem telemetria nem coleta de uso por padrão. O servidor não registra conteúdo; logs só com o necessário para operação e limite de abuso. |
| RNF-24 | O app avisa que o IP fica visível aos participantes (DEC-43). |
| RNF-25 | Permissões do sistema (gravação de tela, microfone, câmera) são pedidas só quando o recurso é usado. |

## 7. Distribuição

| ID | Requisito |
|----|-----------|
| RNF-26 | Um instalador ou binário por sistema, sem exigir runtime externo (a não ser o que o sistema já traz, como PipeWire no Linux). |
| RNF-27 | Build reproduzível em CI para os três sistemas. |
| RNF-28 | No Windows, o app não abre janela de console nem mostra uma ao iniciar processos auxiliares (por exemplo, com `CREATE_NO_WINDOW`). Em builds de depuração ou com `--features console` o console continua disponível para ler logs. |

## 8. Como será validado

- Medições automatizadas de latência, CPU e memória em uma máquina de referência, rodadas a cada marco do MVP.
- Os valores de rede e bitrate saem do ensaio de 1080p60 (item 6 do roteiro); a suposição de ~8 Mbps por espectador será substituída pelo número medido.
- Qualquer meta que se mostre inviável vira ponto para o mantenedor decidir, não é rebaixada em silêncio.
