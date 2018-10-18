use drgporep::{self, DrgPoRep};
use drgraph::Graph;
use error::Result;
use hasher::{Domain, HashFunction, Hasher};
use parameter_cache::ParameterSetIdentifier;
use porep::{self, PoRep};
use proof::ProofScheme;

#[derive(Debug)]
pub struct SetupParams {
    pub drg_porep_setup_params: drgporep::SetupParams,
    pub layers: usize,
}

#[derive(Debug, Clone)]
pub struct PublicParams<H, G>
where
    H: Hasher,
    G: Graph<H> + ParameterSetIdentifier,
{
    pub drg_porep_public_params: drgporep::PublicParams<H, G>,
    pub layers: usize,
}

pub struct Tau<T: Domain> {
    pub layer_taus: Vec<porep::Tau<T>>,
    pub comm_r_star: T,
}

impl<T: Domain> Tau<T> {
    /// Return a single porep::Tau with the initial data and final replica commitments of layer_taus.
    pub fn simplify(&self) -> porep::Tau<T> {
        porep::Tau {
            comm_r: self.layer_taus[self.layer_taus.len() - 1].comm_r,
            comm_d: self.layer_taus[0].comm_d,
        }
    }
}

impl<H, G> ParameterSetIdentifier for PublicParams<H, G>
where
    H: Hasher,
    G: Graph<H> + ParameterSetIdentifier,
{
    fn parameter_set_identifier(&self) -> String {
        format!(
            "layered_drgporep::PublicParams{{ drg_porep_identifier: {}, layers: {} }}",
            self.drg_porep_public_params.parameter_set_identifier(),
            self.layers
        )
    }
}

impl<'a, H, G> From<&'a PublicParams<H, G>> for PublicParams<H, G>
where
    H: Hasher,
    G: Graph<H> + ParameterSetIdentifier,
{
    fn from(pp: &PublicParams<H, G>) -> PublicParams<H, G> {
        PublicParams {
            drg_porep_public_params: pp.drg_porep_public_params.clone(),
            layers: pp.layers,
        }
    }
}

pub type ReplicaParents<H> = Vec<(usize, DataProof<H>)>;
pub type EncodingProof<H> = drgporep::Proof<H>;
pub type DataProof<H> = drgporep::DataProof<H>;

#[derive(Debug)]
pub struct PublicInputs<T: Domain> {
    pub replica_id: T,
    pub challenges: Vec<usize>,
    pub tau: Option<porep::Tau<T>>,
    pub comm_r_star: T,
}

pub struct PrivateInputs<'a, H: Hasher> {
    pub replica: &'a [u8],
    pub aux: Vec<porep::ProverAux<H>>,
    pub tau: Vec<porep::Tau<H::Domain>>,
}

#[derive(Debug, Clone)]
pub struct Proof<H: Hasher> {
    pub encoding_proofs: Vec<EncodingProof<H>>,
    pub tau: Vec<porep::Tau<H::Domain>>,
}

impl<H: Hasher> Proof<H> {
    pub fn new(
        encoding_proofs: Vec<EncodingProof<H>>,
        tau: Vec<porep::Tau<H::Domain>>,
    ) -> Proof<H> {
        Proof {
            encoding_proofs,
            tau,
        }
    }
}

pub trait Layerable<H: Hasher>: Graph<H> {}

/// Layers provides default implementations of methods required to handle proof and verification
/// of layered proofs of replication. Implementations must provide transform and invert_transform methods.
pub trait Layers {
    type Hasher: Hasher;
    type Graph: Layerable<Self::Hasher> + ParameterSetIdentifier;

    /// transform a layer's public parameters, returning new public parameters corresponding to the next layer.
    fn transform(
        pp: &drgporep::PublicParams<Self::Hasher, Self::Graph>,
        layer: usize,
        layers: usize,
    ) -> drgporep::PublicParams<Self::Hasher, Self::Graph>;

    /// transform a layer's public parameters, returning new public parameters corresponding to the previous layer.
    fn invert_transform(
        pp: &drgporep::PublicParams<Self::Hasher, Self::Graph>,
        layer: usize,
        layers: usize,
    ) -> drgporep::PublicParams<Self::Hasher, Self::Graph>;

    fn prove_layers<'a>(
        pp: &drgporep::PublicParams<Self::Hasher, Self::Graph>,
        pub_inputs: &PublicInputs<<Self::Hasher as Hasher>::Domain>,
        priv_inputs: &drgporep::PrivateInputs<Self::Hasher>,
        tau: Vec<porep::Tau<<Self::Hasher as Hasher>::Domain>>,
        aux: Vec<porep::ProverAux<Self::Hasher>>,
        layers: usize,
        total_layers: usize,
        proofs: &'a mut Vec<EncodingProof<Self::Hasher>>,
    ) -> Result<&'a Vec<EncodingProof<Self::Hasher>>> {
        assert!(layers > 0);

        let mut scratch = priv_inputs.replica.to_vec().clone();
        <DrgPoRep<Self::Hasher, Self::Graph> as PoRep<<Self::Hasher as Hasher>::Domain>>::replicate(
            pp,
            &pub_inputs.replica_id,
            scratch.as_mut_slice(),
        )?;

        let new_priv_inputs = drgporep::PrivateInputs {
            replica: scratch.as_slice(),
            // TODO: Make sure this is a shallow clone, not the whole MerkleTree.
            aux: &aux[aux.len() - layers].clone(),
        };
        let drgporep_pub_inputs = drgporep::PublicInputs {
            replica_id: pub_inputs.replica_id,
            challenges: pub_inputs.challenges.clone(),
            tau: Some(tau[tau.len() - layers]),
        };
        let drg_proof = DrgPoRep::prove(&pp, &drgporep_pub_inputs, &new_priv_inputs)?;
        proofs.push(drg_proof);

        let pp = &Self::transform(pp, total_layers - layers, total_layers);

        if layers != 1 {
            Self::prove_layers(
                pp,
                pub_inputs,
                &new_priv_inputs,
                tau,
                aux,
                layers - 1,
                layers,
                proofs,
            )?;
        }

        Ok(proofs)
    }

    fn extract_and_invert_transform_layers<'a>(
        drgpp: &drgporep::PublicParams<Self::Hasher, Self::Graph>,
        layer: usize,
        layers: usize,
        replica_id: &<Self::Hasher as Hasher>::Domain,
        data: &'a mut [u8],
    ) -> Result<()> {
        assert!(layers > 0);

        let inverted = &Self::invert_transform(&drgpp, layer, layers);
        let mut res = DrgPoRep::extract_all(inverted, replica_id, data)?;

        for (i, r) in res.iter_mut().enumerate() {
            data[i] = *r;
        }

        if layers != 1 {
            Self::extract_and_invert_transform_layers(
                inverted,
                layer + 1,
                layers - 1,
                replica_id,
                data,
            )?;
        }

        Ok(())
    }

    fn transform_and_replicate_layers(
        drgpp: &drgporep::PublicParams<Self::Hasher, Self::Graph>,
        layer: usize,
        layers: usize,
        replica_id: &<Self::Hasher as Hasher>::Domain,
        data: &mut [u8],
        taus: &mut Vec<porep::Tau<<Self::Hasher as Hasher>::Domain>>,
        auxs: &mut Vec<porep::ProverAux<Self::Hasher>>,
    ) -> Result<()> {
        assert!(layers > 0);
        let (tau, aux) = DrgPoRep::replicate(drgpp, replica_id, data).unwrap();

        taus.push(tau);
        auxs.push(aux);

        if layers != 1 {
            Self::transform_and_replicate_layers(
                &Self::transform(&drgpp, layer, layers),
                layer + 1,
                layers - 1,
                replica_id,
                data,
                taus,
                auxs,
            )?;
        }

        Ok(())
    }
}

impl<'a, L: Layers> ProofScheme<'a> for L {
    type PublicParams = PublicParams<L::Hasher, L::Graph>;
    type SetupParams = SetupParams;
    type PublicInputs = PublicInputs<<L::Hasher as Hasher>::Domain>;
    type PrivateInputs = PrivateInputs<'a, L::Hasher>;
    type Proof = Proof<L::Hasher>;

    fn setup(sp: &Self::SetupParams) -> Result<Self::PublicParams> {
        let dp_sp = DrgPoRep::setup(&sp.drg_porep_setup_params)?;
        let pp = PublicParams {
            drg_porep_public_params: dp_sp,
            layers: sp.layers,
        };

        Ok(pp)
    }

    fn prove<'b>(
        pub_params: &'b Self::PublicParams,
        pub_inputs: &'b Self::PublicInputs,
        priv_inputs: &'b Self::PrivateInputs,
    ) -> Result<Self::Proof> {
        let drg_priv_inputs = drgporep::PrivateInputs {
            aux: &priv_inputs.aux[0].clone(),
            replica: priv_inputs.replica,
        };

        let mut proofs = Vec::with_capacity(pub_params.layers);

        Self::prove_layers(
            &pub_params.drg_porep_public_params,
            pub_inputs,
            &drg_priv_inputs,
            priv_inputs.tau.clone(),
            priv_inputs.aux.clone(),
            pub_params.layers,
            pub_params.layers,
            &mut proofs,
        )?;

        // We need to calculate CommR* -- which is: H(replica_id|comm_r[0]|comm_r[1]|…comm_r[n])
        let proof = Proof::new(proofs, priv_inputs.tau.clone());

        Ok(proof)
    }

    fn verify(
        pub_params: &Self::PublicParams,
        pub_inputs: &Self::PublicInputs,
        proof: &Self::Proof,
    ) -> Result<bool> {
        if proof.encoding_proofs.len() != pub_params.layers {
            return Ok(false);
        }

        let total_layers = pub_params.layers;
        let mut pp = pub_params.drg_porep_public_params.clone();
        // TODO: verification is broken for the first node, figure out how to unbreak
        // with permutations

        let mut comm_rs = Vec::new();

        for (layer, proof_layer) in proof.encoding_proofs.iter().enumerate() {
            comm_rs.push(proof.tau[layer].comm_r);

            let new_pub_inputs = drgporep::PublicInputs {
                replica_id: pub_inputs.replica_id,
                challenges: pub_inputs.challenges.clone(),
                tau: Some(proof.tau[layer]),
            };

            let ep = &proof_layer;
            let parents: Vec<_> = ep.replica_parents[0]
                .iter()
                .map(|p| {
                    (
                        p.0,
                        drgporep::DataProof {
                            // TODO: investigate if clone can be avoided by using a reference in drgporep::DataProof
                            proof: p.1.proof.clone(),
                            data: p.1.data,
                        },
                    )
                })
                .collect();

            let res = DrgPoRep::verify(
                &pp,
                &new_pub_inputs,
                &drgporep::Proof {
                    replica_nodes: vec![drgporep::DataProof {
                        // TODO: investigate if clone can be avoided by using a reference in drgporep::DataProof
                        proof: ep.replica_nodes[0].proof.clone(),
                        data: ep.replica_nodes[0].data,
                    }],
                    replica_parents: vec![parents],
                    // TODO: investigate if clone can be avoided by using a reference in drgporep::DataProof
                    nodes: vec![ep.nodes[0].clone()],
                },
            )?;

            pp = Self::transform(&pp, layer, total_layers);

            if !res {
                return Ok(false);
            }
        }
        let crs = comm_r_star::<L::Hasher>(&pub_inputs.replica_id, &comm_rs)?;

        Ok(crs == pub_inputs.comm_r_star)
    }
}

fn comm_r_star<H: Hasher>(replica_id: &H::Domain, comm_rs: &[H::Domain]) -> Result<H::Domain> {
    let l = (comm_rs.len() + 1) * 32;
    let mut bytes = vec![0; l];

    replica_id.write_bytes(&mut bytes[0..32])?;

    for (i, comm_r) in comm_rs.iter().enumerate() {
        comm_r.write_bytes(&mut bytes[(i + 1) * 32..(i + 2) * 32])?;
    }

    Ok(H::Function::hash(&bytes))
}

impl<'a, 'c, L: Layers> PoRep<'a, <L::Hasher as Hasher>::Domain> for L {
    type Tau = Tau<<L::Hasher as Hasher>::Domain>;
    type ProverAux = Vec<porep::ProverAux<L::Hasher>>;

    fn replicate(
        pp: &'a PublicParams<L::Hasher, L::Graph>,
        replica_id: &<L::Hasher as Hasher>::Domain,
        data: &mut [u8],
    ) -> Result<(Self::Tau, Self::ProverAux)> {
        let mut taus = Vec::with_capacity(pp.layers);
        let mut auxs = Vec::with_capacity(pp.layers);

        Self::transform_and_replicate_layers(
            &pp.drg_porep_public_params,
            0,
            pp.layers,
            replica_id,
            data,
            &mut taus,
            &mut auxs,
        )?;

        let comm_rs: Vec<_> = taus.iter().map(|tau| tau.comm_r).collect();
        let crs = comm_r_star::<L::Hasher>(replica_id, &comm_rs)?;
        let tau = Tau {
            layer_taus: taus,
            comm_r_star: crs,
        };
        Ok((tau, auxs))
    }

    fn extract_all<'b>(
        pp: &'b PublicParams<L::Hasher, L::Graph>,
        replica_id: &'b <L::Hasher as Hasher>::Domain,
        data: &'b [u8],
    ) -> Result<Vec<u8>> {
        let mut data = data.to_vec();

        Self::extract_and_invert_transform_layers(
            &pp.drg_porep_public_params,
            0,
            pp.layers,
            replica_id,
            &mut data,
        )?;

        Ok(data)
    }

    fn extract(
        _pp: &PublicParams<L::Hasher, L::Graph>,
        _replica_id: &<L::Hasher as Hasher>::Domain,
        _data: &[u8],
        _node: usize,
    ) -> Result<Vec<u8>> {
        unimplemented!();
    }
}