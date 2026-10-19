// Local Source analysis only. Output is Mojang-derived, never tracked or distributed.
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.jar.*;
import net.minecraft.client.animation.*;
import net.minecraft.client.model.Model;
import net.minecraft.client.model.geom.*;
import net.minecraft.client.renderer.entity.*;
import net.minecraft.core.registries.BuiltInRegistries;
import com.google.gson.Gson;

public final class ExportPresentation {
 static Map<String,Object> roots = new TreeMap<>();
 static void partPaths(ModelPart part,List<String> path,IdentityHashMap<ModelPart,List<List<String>>> paths,Field children) throws Exception {
  paths.computeIfAbsent(part,ignored->new ArrayList<>()).add(List.copyOf(path));
  for(var entry:((Map<String,ModelPart>)children.get(part)).entrySet()) {
   var childPath=new ArrayList<>(path);childPath.add(entry.getKey());
   partPaths(entry.getValue(),childPath,paths,children);
  }
 }
 static Object partField(Object value,IdentityHashMap<ModelPart,List<List<String>>> paths) {
  if(value instanceof ModelPart part) {
   var locations=paths.get(part);
   return locations==null?null:Map.of("paths",locations);
  }
  if(value instanceof ModelPart[] array) {
   var result=new ArrayList<Object>();
   for(ModelPart part:array)result.add(partField(part,paths));
   return result;
  }
  return null;
 }
 static Map<String,Object> partFields(Model model) throws Exception {
  Field children=ModelPart.class.getDeclaredField("children");children.setAccessible(true);
  var paths=new IdentityHashMap<ModelPart,List<List<String>>>();
  partPaths(model.root(),List.of(),paths,children);
  var result=new TreeMap<String,Object>();
  for(Class<?> c=model.getClass();c!=null;c=c.getSuperclass())for(Field field:c.getDeclaredFields()) {
   if(Modifier.isStatic(field.getModifiers()))continue;
   if(field.getType()!=ModelPart.class&&field.getType()!=ModelPart[].class)continue;
   field.setAccessible(true);
   Object captured=partField(field.get(model),paths);
   if(captured!=null)result.putIfAbsent(field.getName(),captured);
  }
  return result;
 }
 static Object material(Object value,String key,int depth) throws Exception {
  if(value==null||depth>8)return null;
  if(value instanceof Model model) {
   roots.put(key,ExportModels.part("root",model.root()));
   return Map.of("rig",key,"class",model.getClass().getName(),"part_fields",partFields(model));
  }
  if(value instanceof net.minecraft.resources.Identifier || value instanceof Number || value instanceof Boolean || value instanceof String) return value instanceof net.minecraft.resources.Identifier?value.toString():value;
  if(value instanceof Map<?,?> map) {
   var result=new TreeMap<String,Object>();
   var ids=new TreeMap<String,String>();
   for(var entry:map.entrySet()) {
    String name=entry.getKey().toString();
    Object item=material(entry.getValue(),key+"/"+name,depth+1);
    if(item!=null)result.put(name,item);
    if(entry.getKey() instanceof Enum<?> e)ids.put(Integer.toString(ExportModels.enumId(e)),name);
   }
   if(!ids.isEmpty())result.put("__ids",ids);
   return result.isEmpty()?null:result;
  }
  if(value.getClass().isRecord()) {
   var result=new TreeMap<String,Object>();
   for(var component:value.getClass().getRecordComponents()) {
    Method method=component.getAccessor();method.setAccessible(true);
    Object item=material(method.invoke(value),key+"/"+component.getName(),depth+1);
    if(item!=null)result.put(component.getName(),item);
   }
   return result.isEmpty()?null:result;
  }
  if(value.getClass().isArray()) {
   var result=new ArrayList<Object>();
   for(int i=0;i<Array.getLength(value);i++)result.add(material(Array.get(value,i),key+"/"+i,depth+1));
   return result;
  }
  return null;
 }
 static Map<String,Object> fields(Object owner,String key) throws Exception {
  var result=new TreeMap<String,Object>();
  for(Class<?> c=owner.getClass();c!=null;c=c.getSuperclass()) for(Field f:c.getDeclaredFields()) {
   f.setAccessible(true);
   Object value=f.get(Modifier.isStatic(f.getModifiers())?null:owner);
   Object material=material(value,key+"/"+f.getName(),0);
   if(material!=null)result.putIfAbsent(f.getName(),material);
  }
  return result;
 }
 static List<Object> layers(Object renderer,String name) throws Exception {
  Field field=LivingEntityRenderer.class.getDeclaredField("layers");field.setAccessible(true);
  List<?> layers=(List<?>)field.get(renderer);
  List<Object> results=new ArrayList<>();
  int index=0;
  for(Object layer:layers) {
   var out=new TreeMap<String,Object>();
   out.put("class",layer.getClass().getName());
   out.put("fields",fields(layer,name+"#layer"+(index++)));
   results.add(out);
  }
  return results;
 }
 static float[] vector(org.joml.Vector3fc v){ return new float[]{v.x(),v.y(),v.z()}; }
 static Object clip(AnimationDefinition definition) {
  var bones=new TreeMap<String,Object>();
  for(var entry:definition.boneAnimations().entrySet()) {
   List<Object> channels=new ArrayList<>();
   for(var channel:entry.getValue()) {
    String target=channel.target()==AnimationChannel.Targets.POSITION?"position":channel.target()==AnimationChannel.Targets.ROTATION?"rotation":channel.target()==AnimationChannel.Targets.SCALE?"scale":null;
    if(target==null) throw new IllegalStateException("unknown animation target");
    List<Object> keys=new ArrayList<>();
    for(var key:channel.keyframes()) {
     String interpolation=key.interpolation()==AnimationChannel.Interpolations.LINEAR?"linear":key.interpolation()==AnimationChannel.Interpolations.CATMULLROM?"catmullrom":null;
     if(interpolation==null) throw new IllegalStateException("unknown interpolation");
     keys.add(Map.of("time",key.timestamp(),"pre",vector(key.preTarget()),"post",vector(key.postTarget()),"interpolation",interpolation));
    }
    channels.add(Map.of("target",target,"keys",keys));
   }
   bones.put(entry.getKey(),channels);
  }
  return Map.of("length",definition.lengthInSeconds(),"looping",definition.looping(),"bones",bones);
 }
 public static void main(String[] args) throws Exception {
  net.minecraft.SharedConstants.tryDetectVersion();net.minecraft.server.Bootstrap.bootStrap();
  Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");uf.setAccessible(true);
  var unsafe=(sun.misc.Unsafe)uf.get(null);
  var context=(EntityRendererProvider.Context)unsafe.allocateInstance(EntityRendererProvider.Context.class);
  Field mf=EntityRendererProvider.Context.class.getDeclaredField("modelSet");mf.setAccessible(true);mf.set(context,EntityModelSet.vanilla());
  Field pf=EntityRenderers.class.getDeclaredField("PROVIDERS");pf.setAccessible(true);
  var families=new TreeMap<String,Object>();
  for(var entry:((Map<?,?>)pf.get(null)).entrySet()) {
   String name=BuiltInRegistries.ENTITY_TYPE.getKey((net.minecraft.world.entity.EntityType<?>)entry.getKey()).toString();
   var result=new TreeMap<String,Object>();
   try {
    var renderer=((EntityRendererProvider<?>)entry.getValue()).create(context);
    result.put("class",renderer.getClass().getName());
    result.put("fields",fields(renderer,name+"#presentation"));
    if(renderer instanceof LivingEntityRenderer living) {
     result.put("model_class",living.getModel().getClass().getName());
     result.put("layers",layers(renderer,name));
    }
   } catch(Throwable error) {result.put("error",error.toString());}
   families.put(name,result);
  }
  // Avatar variants have distinct rigs, not extra ENTITY_TYPE holders. Both
  // player and mannequin dispatch through createAvatarRenderers in 26.1.2.
  for(String avatar:new String[]{"minecraft:player","minecraft:mannequin"}) {
   for(boolean slim:new boolean[]{false,true}) {
    String rig=avatar+(slim?"_slim":"");
    var renderer=new net.minecraft.client.renderer.entity.player.AvatarRenderer(context,slim);
    var family=Map.of("class",renderer.getClass().getName(),"model_class",renderer.getModel().getClass().getName(),"fields",fields(renderer,rig+"#presentation"),"layers",layers(renderer,rig));
    if(!slim) families.put(avatar,family);
   }
  }
  var holders=new TreeSet<String>();
  for(var type:BuiltInRegistries.ENTITY_TYPE) holders.add(BuiltInRegistries.ENTITY_TYPE.getKey(type).toString());
  if(!families.keySet().equals(holders)) throw new IllegalStateException("entity presentation holder coverage mismatch");
  var animations=new TreeMap<String,Object>();
  try(var jar=new JarFile(args[0])) {
   var entries=jar.entries();
   while(entries.hasMoreElements()) {
    String name=entries.nextElement().getName();
    if(!name.startsWith("net/minecraft/client/animation/definitions/")||!name.endsWith(".class")||name.contains("$"))continue;
    Class<?> type=Class.forName(name.substring(0,name.length()-6).replace('/','.'));
    for(Field field:type.getDeclaredFields()) if(Modifier.isStatic(field.getModifiers())&&field.getType()==AnimationDefinition.class) {
     field.setAccessible(true);animations.put(type.getName()+"#"+field.getName(),clip((AnimationDefinition)field.get(null)));
    }
   }
  }
  String clientSha=HexFormat.of().formatHex(java.security.MessageDigest.getInstance("SHA-256").digest(Files.readAllBytes(Path.of(args[0]))));
  Files.writeString(Path.of(args[1]),new Gson().toJson(Map.of("schema",2,"game_version","26.1.2","client_sha256",clientSha,"families",families,"animations",animations,"roots",roots)));
  System.out.println("presentation families="+families.size()+" animations="+animations.size()+" roots="+roots.size());
 }
}
